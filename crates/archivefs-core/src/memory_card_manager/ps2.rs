//! PS2 cards. The filesystem is read, exported (`.psu`) and written by
//! EmuWiz's existing code in [`crate::memory_card_inventory`]; nothing here
//! re-implements it. The `ps2-memcard` crate is the second reader and
//! supplies the spare-area ECC that PCSX2's own `.ps2` cards carry.

use super::{
    CardError, CardFormat, CardHealth, CardReport, CardSave, ImportOutcome, SaveExport,
    decode_title, io_error, product_code_and_region, write_new_file,
};
use crate::memory_card_inventory::{
    MemoryCardHealth, MemoryCardInventory, Ps2DirectoryEntryKind, Ps2PageRepresentation,
    apply_ps2_psu_export, build_restored_card, inspect_memory_card, inspect_memory_card_bytes,
    plan_ps2_psu_export, plan_ps2_psu_restore, restore_free_clusters,
};
use ps2_memcard::{PAGE, RAW_PAGE, SPARE};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// (save name, [(file name, size)]) as each reader sees the card.
type Listing = BTreeMap<String, Vec<(String, u64)>>;

fn own_listing(own: &MemoryCardInventory) -> Listing {
    let mut listing = Listing::new();
    if let Some(inventory) = &own.ps2_inventory {
        for save in &inventory.save_directories {
            let mut files: Vec<(String, u64)> = save
                .files
                .iter()
                .map(|file| (file.entry.display_name.clone(), file.declared_size_bytes))
                .collect();
            files.sort();
            listing.insert(save.entry.display_name.clone(), files);
        }
    }
    listing
}

/// Root folders EmuWiz's own reader sees but deliberately does not open as
/// saves: the console's own settings (`BEDATA-SYSTEM`).
fn unopened_folders(own: &MemoryCardInventory) -> Vec<String> {
    let Some(inventory) = &own.ps2_inventory else {
        return Vec::new();
    };
    inventory
        .root_entries
        .iter()
        .filter(|entry| {
            entry.kind == Ps2DirectoryEntryKind::Directory
                && entry.display_name != "."
                && entry.display_name != ".."
                && !inventory
                    .save_directories
                    .iter()
                    .any(|save| save.entry.raw_entry_offset == entry.raw_entry_offset)
        })
        .map(|entry| entry.display_name.clone())
        .collect()
}

fn library_listing(saves: &[ps2_memcard::Save], unopened: &[String]) -> Listing {
    saves
        .iter()
        .filter(|save| !unopened.contains(&save.name))
        .map(|save| {
            let mut files: Vec<(String, u64)> = save
                .files
                .iter()
                .map(|file| (file.name.clone(), file.data.len() as u64))
                .collect();
            files.sort();
            (save.name.clone(), files)
        })
        .collect()
}

/// Where two listings differ, in a sentence short enough to show.
fn listing_difference(ours: &Listing, theirs: &Listing) -> String {
    let mut differences = Vec::new();
    for name in ours.keys().chain(theirs.keys()) {
        let line = match (ours.get(name), theirs.get(name)) {
            (Some(_), None) => format!("{name}: only EmuWiz's reader lists it"),
            (None, Some(_)) => format!("{name}: only the ps2-memcard reader lists it"),
            (Some(left), Some(right)) if left != right => {
                let odd = left
                    .iter()
                    .find(|file| !right.contains(file))
                    .or_else(|| right.iter().find(|file| !left.contains(file)));
                format!(
                    "{name}: {} file(s) against {}{}",
                    left.len(),
                    right.len(),
                    odd.map(|(file, size)| format!(", first difference {file} ({size} bytes)"))
                        .unwrap_or_default()
                )
            }
            _ => continue,
        };
        if !differences.contains(&line) {
            differences.push(line);
        }
    }
    differences.truncate(4);
    format!("Differences: {}.", differences.join("; "))
}

/// The title a save's `icon.sys` carries (Shift-JIS at 0xC0, 68 bytes; the
/// 16-bit value at 6 is where its second line starts).
pub(super) fn icon_sys_title(icon_sys: &[u8]) -> Option<String> {
    if !icon_sys.starts_with(b"PS2D") {
        return None;
    }
    let field = icon_sys.get(0xC0..0xC0 + 68)?;
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    let split = usize::from(u16::from_le_bytes([icon_sys[6], icon_sys[7]]));
    if split > 0 && split < end {
        let first = decode_title(&field[..split]).unwrap_or_default();
        let second = decode_title(&field[split..end]).unwrap_or_default();
        let joined = format!("{first} {second}");
        let joined = joined.trim();
        return (!joined.is_empty()).then(|| joined.to_string());
    }
    decode_title(&field[..end])
}

/// PCSX2 keeps an 8-byte change marker of its own at file offset 0x210, the
/// start of the card's second page. It is not part of the PS2 filesystem and
/// PCSX2 writes it without touching that page's ECC. PCSX2 compares it with
/// the value in a save state to decide whether the card was swapped.
const PCSX2_MARKER_PAGE: usize = 1;
const PCSX2_MARKER_BYTES: usize = 8;

/// How the `ps2-memcard` ECC compares with the ECC already on the card:
/// (pages whose stored code matches, written pages checked). The page that
/// holds PCSX2's marker is checked as the console wrote it, without the marker.
pub(crate) fn ecc_agreement(bytes: &[u8]) -> (usize, usize) {
    let mut matching = 0;
    let mut written = 0;
    let mut expected = [0u8; SPARE];
    for (index, page) in bytes.chunks_exact(RAW_PAGE).enumerate() {
        // An erased page (never written since the last format) carries no code.
        if page.iter().all(|byte| *byte == 0xFF) {
            continue;
        }
        written += 1;
        ps2_memcard::ecc::fill_spare(&page[..PAGE], &mut expected);
        let mut agrees = page[PAGE..PAGE + 12] == expected[..12];
        if !agrees && index == PCSX2_MARKER_PAGE {
            let mut without_marker = page[..PAGE].to_vec();
            without_marker[..PCSX2_MARKER_BYTES].fill(0xFF);
            ps2_memcard::ecc::fill_spare(&without_marker, &mut expected);
            agrees = page[PAGE..PAGE + 12] == expected[..12];
        }
        if agrees {
            matching += 1;
        }
    }
    (matching, written)
}

pub(super) fn report(path: &Path, bytes: &[u8], sha256: String) -> CardReport {
    let own = inspect_memory_card_bytes(path, bytes);
    let with_ecc = own
        .ps2_geometry
        .as_ref()
        .is_some_and(|geometry| geometry.representation == Ps2PageRepresentation::RawWithSpare);
    let format = if with_ecc {
        CardFormat::Ps2WithEcc
    } else {
        CardFormat::Ps2DataOnly
    };
    let mut notes = Vec::new();
    let mut unsupported = vec![
        "PS2 saves other than .psu (.max, .cbs, .sps/.xps, .psv) cannot be imported.".to_string(),
    ];
    let mut health = match own.health {
        MemoryCardHealth::Healthy => CardHealth::Healthy,
        _ if own.ps2_inventory.is_some() => CardHealth::Warnings,
        _ => CardHealth::Unreadable,
    };
    if health != CardHealth::Healthy {
        notes.extend(own.warnings.iter().cloned());
    }

    let library = ps2_memcard::MemoryCard::parse(bytes).and_then(|card| card.saves());
    let unopened = unopened_folders(&own);
    let mut titles: BTreeMap<String, String> = BTreeMap::new();
    let readers_agree = match &library {
        Ok(saves) => {
            for save in saves {
                if let Some(title) = save
                    .files
                    .iter()
                    .find(|file| file.name == "icon.sys")
                    .and_then(|file| icon_sys_title(&file.data))
                {
                    titles.insert(save.name.clone(), title);
                }
            }
            let (ours, theirs) = (own_listing(&own), library_listing(saves, &unopened));
            let agree = ours == theirs;
            if !agree && health != CardHealth::Unreadable {
                health = CardHealth::Warnings;
                notes.push(format!(
                    "EmuWiz's reader and the ps2-memcard reader list different saves or files \
                     on this card, so nothing is exported from or imported into it. {}",
                    listing_difference(&ours, &theirs)
                ));
            }
            Some(agree)
        }
        Err(error) => {
            if health == CardHealth::Healthy {
                health = CardHealth::Warnings;
            }
            notes.push(format!(
                "The ps2-memcard reader refused this card: {error}."
            ));
            Some(false)
        }
    };

    let mut ecc_writable = true;
    if with_ecc {
        let (matching, written) = ecc_agreement(bytes);
        if matching != written {
            ecc_writable = false;
            unsupported.push(format!(
                "The error-correction codes on {} of this card's {written} written pages are \
                 not the ones EmuWiz would compute, so EmuWiz will not write a copy of this \
                 card (saves can still be exported).",
                written - matching
            ));
        }
    }

    let mut saves = Vec::new();
    let mut free_space = None;
    if let (Some(inventory), Some(geometry)) = (&own.ps2_inventory, &own.ps2_geometry) {
        for save in &inventory.save_directories {
            let name = save.entry.display_name.clone();
            let (product_code, region) = product_code_and_region(&name);
            saves.push(CardSave {
                title: titles.get(&name).cloned(),
                product_code,
                region,
                size_bytes: save.files.iter().map(|file| file.declared_size_bytes).sum(),
                blocks: None,
                file_count: Some(save.files.len()),
                modified: save.entry.modified.as_ref().map(|time| {
                    format!(
                        "{:04}-{:02}-{:02} {:02}:{:02}",
                        time.year, time.month, time.day, time.hour, time.minute
                    )
                }),
                notes: save
                    .warnings
                    .iter()
                    .map(|warning| warning.message.clone())
                    .collect(),
                name,
            });
        }
        // The console's own settings folder: EmuWiz's reader leaves it
        // closed, so what is shown comes from the second reader. Shown only.
        for save in library
            .iter()
            .flatten()
            .filter(|save| unopened.contains(&save.name))
        {
            saves.push(CardSave {
                name: save.name.clone(),
                product_code: None,
                region: None,
                title: titles.get(&save.name).cloned(),
                size_bytes: save.len() as u64,
                blocks: None,
                file_count: Some(save.files.len()),
                modified: None,
                notes: vec![
                    "The console's own settings, not a game save. Shown only: it is not \
                     exported."
                        .into(),
                ],
            });
        }
        let cluster_bytes =
            u64::from(geometry.pages_per_cluster) * u64::from(geometry.page_data_bytes);
        if let Ok(free) = restore_free_clusters(bytes, geometry, inventory, None) {
            free_space = Some(format!(
                "{} KB free",
                free.len() as u64 * cluster_bytes / 1024
            ));
        }
    }
    let healthy = health == CardHealth::Healthy;
    CardReport {
        path: path.to_path_buf(),
        format,
        size_bytes: bytes.len() as u64,
        sha256: Some(sha256),
        health,
        saves,
        free_space,
        readers_agree,
        export_format: healthy.then_some("psu"),
        import_formats: if healthy && ecc_writable {
            vec!["psu"]
        } else {
            Vec::new()
        },
        notes,
        unsupported,
    }
}

/// A PCSX2 folder card: listed from the folder, never opened as a filesystem.
pub(super) fn folder_report(path: &Path) -> Result<CardReport, CardError> {
    let marker = path.join("_pcsx2_superblock");
    if !fs::symlink_metadata(&marker).is_ok_and(|metadata| metadata.is_file()) {
        return Err(CardError::NotAFile(path.to_path_buf()));
    }
    let mut saves = Vec::new();
    let entries = fs::read_dir(path).map_err(|error| io_error("cannot list", path, error))?;
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let mut size_bytes = 0;
        let mut file_count = 0;
        if let Ok(files) = fs::read_dir(entry.path()) {
            for file in files.flatten() {
                let file_name = file.file_name();
                if file_name.to_string_lossy().starts_with("_pcsx2_") {
                    continue;
                }
                if let Ok(metadata) = fs::symlink_metadata(file.path())
                    && metadata.is_file()
                {
                    size_bytes += metadata.len();
                    file_count += 1;
                }
            }
        }
        let icon = entry.path().join("icon.sys");
        let title = fs::symlink_metadata(&icon)
            .ok()
            .filter(|metadata| metadata.is_file() && metadata.len() <= 4096)
            .and_then(|_| fs::read(&icon).ok())
            .and_then(|bytes| icon_sys_title(&bytes));
        let (product_code, region) = product_code_and_region(&name);
        saves.push(CardSave {
            name,
            product_code,
            region,
            title,
            size_bytes,
            blocks: None,
            file_count: Some(file_count),
            modified: None,
            notes: Vec::new(),
        });
    }
    saves.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(CardReport {
        path: path.to_path_buf(),
        format: CardFormat::Pcsx2Folder,
        size_bytes: saves.iter().map(|save| save.size_bytes).sum(),
        sha256: None,
        health: CardHealth::Healthy,
        saves,
        free_space: None,
        readers_agree: None,
        export_format: None,
        import_formats: Vec::new(),
        notes: Vec::new(),
        unsupported: vec![
            "PCSX2 folder cards are listed only: EmuWiz does not back them up, export from \
             them or import into them."
                .into(),
        ],
    })
}

/// A `.psu` save: the save folder's record, its `.` and `..` records, and
/// each file's record with its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Psu {
    pub root: Vec<u8>,
    pub dot: Vec<u8>,
    pub dotdot: Vec<u8>,
    pub files: Vec<(Vec<u8>, Vec<u8>)>,
}

const PSU_ENTRY: usize = 512;
const PSU_ALIGN: usize = 1024;
const MODE_FILE: u16 = 0x0010;
const MODE_DIRECTORY: u16 = 0x0020;
const MODE_EXISTS: u16 = 0x8000;

fn psu_mode(entry: &[u8]) -> u16 {
    u16::from_le_bytes([entry[0], entry[1]])
}

fn psu_length(entry: &[u8]) -> usize {
    u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]) as usize
}

fn psu_name(entry: &[u8]) -> &[u8] {
    let field = &entry[0x40..0x60];
    &field[..field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len())]
}

fn psu_is_file(entry: &[u8]) -> bool {
    let mode = psu_mode(entry);
    mode & MODE_EXISTS != 0
        && mode & MODE_FILE != 0
        && mode & MODE_DIRECTORY == 0
        && !psu_name(entry).is_empty()
}

/// The three leading records every `.psu` has, and how many files follow.
fn psu_head(bytes: &[u8]) -> Option<(Psu, usize)> {
    let record = |index: usize| bytes.get(index * PSU_ENTRY..(index + 1) * PSU_ENTRY);
    let (root, dot, dotdot) = (record(0)?, record(1)?, record(2)?);
    let is_directory =
        |entry: &[u8]| psu_mode(entry) & MODE_EXISTS != 0 && psu_mode(entry) & MODE_DIRECTORY != 0;
    if !is_directory(root) || psu_name(dot) != b"." || psu_name(dotdot) != b".." {
        return None;
    }
    let files = psu_length(root).checked_sub(2)?;
    if files > crate::memory_card_inventory::PS2_PSU_MAX_FILES {
        return None;
    }
    Some((
        Psu {
            root: root.to_vec(),
            dot: dot.to_vec(),
            dotdot: dotdot.to_vec(),
            files: Vec::new(),
        },
        files,
    ))
}

/// The standard layout (EMS adapter, uLaunchELF, mymc, PCSX2 tools): each
/// file's record is followed directly by its bytes, padded to 1024.
pub(super) fn parse_psu_standard(bytes: &[u8]) -> Option<Psu> {
    let (mut psu, files) = psu_head(bytes)?;
    let mut offset = PSU_ENTRY * 3;
    for index in 0..files {
        let entry = bytes.get(offset..offset + PSU_ENTRY)?;
        if !psu_is_file(entry) {
            return None;
        }
        let length = psu_length(entry);
        offset += PSU_ENTRY;
        let data = bytes.get(offset..offset.checked_add(length)?)?;
        psu.files.push((entry.to_vec(), data.to_vec()));
        offset += length;
        // Each file is padded to a multiple of 1024 of its own length. Some
        // writers leave the last file unpadded.
        let padded = offset + psu_padding(length);
        offset = if index + 1 == files && bytes.len() < padded {
            offset
        } else {
            padded
        };
    }
    (offset == bytes.len()).then_some(psu)
}

/// The layout EmuWiz's PS2 export wrote in earlier releases: every record
/// first, then every file's bytes. Identical to the standard layout for a
/// save with one file, and not readable by other tools otherwise.
pub(super) fn parse_psu_records_first(bytes: &[u8]) -> Option<Psu> {
    let (mut psu, files) = psu_head(bytes)?;
    let mut offset = PSU_ENTRY * (3 + files);
    for index in 0..files {
        let entry = bytes.get(PSU_ENTRY * (3 + index)..PSU_ENTRY * (4 + index))?;
        if !psu_is_file(entry) {
            return None;
        }
        let length = psu_length(entry);
        let data = bytes.get(offset..offset.checked_add(length)?)?;
        psu.files.push((entry.to_vec(), data.to_vec()));
        offset += length.div_ceil(PSU_ALIGN) * PSU_ALIGN;
    }
    (offset == bytes.len()).then_some(psu)
}

fn psu_padding(length: usize) -> usize {
    (PSU_ALIGN - length % PSU_ALIGN) % PSU_ALIGN
}

fn psu_padded(data: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(data);
    out.resize(out.len() + psu_padding(data.len()), 0);
}

pub(super) fn write_psu_standard(psu: &Psu) -> Vec<u8> {
    let mut out = [&psu.root[..], &psu.dot[..], &psu.dotdot[..]].concat();
    for (entry, data) in &psu.files {
        out.extend_from_slice(entry);
        psu_padded(data, &mut out);
    }
    out
}

#[cfg(test)]
pub(super) fn write_psu_records_first(psu: &Psu) -> Vec<u8> {
    let mut out = [&psu.root[..], &psu.dot[..], &psu.dotdot[..]].concat();
    for (entry, _) in &psu.files {
        out.extend_from_slice(entry);
    }
    for (_, data) in &psu.files {
        psu_padded(data, &mut out);
    }
    out
}

/// (name, bytes) of every file, sorted: what must survive a round trip.
fn psu_contents(psu: &Psu) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut files: Vec<(Vec<u8>, Vec<u8>)> = psu
        .files
        .iter()
        .map(|(entry, data)| (psu_name(entry).to_vec(), data.clone()))
        .collect();
    files.sort();
    files
}

/// A private working folder beside `near`, removed when dropped. The path
/// has no links in it, which the existing planner requires.
fn working_folder(near: &Path) -> Result<tempfile::TempDir, CardError> {
    let parent = near
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| CardError::Refused("the new file needs a folder to be written in".into()))?;
    let parent = fs::canonicalize(parent).map_err(|error| io_error("cannot use", parent, error))?;
    tempfile::Builder::new()
        .prefix(".emuwiz-card-work-")
        .tempdir_in(&parent)
        .map_err(|error| io_error("cannot create a working folder in", &parent, error))
}

/// Reads one save out of a card with EmuWiz's existing exporter.
fn read_save(
    card: &Path,
    card_bytes: &[u8],
    save_name: &str,
    work: &Path,
) -> Result<Psu, CardError> {
    let own = inspect_memory_card_bytes(card, card_bytes);
    if unopened_folders(&own).iter().any(|name| name == save_name) {
        return Err(CardError::Refused(format!(
            "{save_name} is the console's own settings folder, not a game save; EmuWiz does \
             not export it"
        )));
    }
    let save = own
        .ps2_inventory
        .as_ref()
        .and_then(|inventory| {
            inventory
                .save_directories
                .iter()
                .find(|save| save.entry.display_name == save_name)
        })
        .ok_or_else(|| CardError::SaveNotFound(save_name.to_string()))?;
    let staged = work.join("staged.psu");
    let _ = fs::remove_file(&staged);
    let plan = plan_ps2_psu_export(&own, save, &staged)
        .map_err(|error| CardError::Refused(error.to_string()))?;
    apply_ps2_psu_export(&plan).map_err(|error| CardError::Refused(error.to_string()))?;
    let bytes = fs::read(&staged).map_err(|error| io_error("cannot read", &staged, error))?;
    let _ = fs::remove_file(&staged);
    parse_psu_standard(&bytes).ok_or_else(|| {
        CardError::VerificationFailed("the save read from the card is not a complete .psu".into())
    })
}

/// Exports one save as a standard `.psu`.
pub(super) fn export_psu(
    card: &Path,
    card_bytes: &[u8],
    save_name: &str,
    destination: &Path,
    source_card_sha256: String,
) -> Result<SaveExport, CardError> {
    if fs::symlink_metadata(destination).is_ok() {
        return Err(CardError::DestinationExists(destination.to_path_buf()));
    }
    let work = working_folder(destination)?;
    let psu = read_save(card, card_bytes, save_name, work.path())?;
    let bytes = write_psu_standard(&psu);
    // What is written must read back as the same files in the standard layout.
    if parse_psu_standard(&bytes).map(|back| psu_contents(&back)) != Some(psu_contents(&psu)) {
        return Err(CardError::VerificationFailed(
            "the exported save does not read back as the save on the card".into(),
        ));
    }
    write_new_file(destination, &bytes)?;
    Ok(SaveExport {
        save_name: save_name.to_string(),
        destination: destination.to_path_buf(),
        format: "psu",
        bytes: bytes.len() as u64,
        sha256: super::sha256_hex(&bytes),
        source_card: card.to_path_buf(),
        source_card_sha256,
    })
}

fn strip_spare(bytes: &[u8]) -> Result<Vec<u8>, CardError> {
    if !bytes.len().is_multiple_of(RAW_PAGE) {
        return Err(CardError::Refused(
            "the card is not a whole number of 528-byte pages".into(),
        ));
    }
    Ok(bytes
        .chunks_exact(RAW_PAGE)
        .flat_map(|page| &page[..PAGE])
        .copied()
        .collect())
}

/// A copy of the card with the `.psu` save added.
///
/// The filesystem change is computed by EmuWiz's existing restore code on a
/// private data-only working copy. For a card with ECC, pages whose data did
/// not change are carried over exactly as they were, and changed pages get a
/// freshly computed code.
pub(super) fn import_into_copy(
    card: &Path,
    card_bytes: &[u8],
    save_file: &Path,
    new_card: &Path,
) -> Result<ImportOutcome, CardError> {
    let save_bytes = super::read_card_bytes(save_file)?;
    let incoming = parse_psu_standard(&save_bytes)
        .or_else(|| parse_psu_records_first(&save_bytes))
        .ok_or_else(|| {
            CardError::Unsupported(
                "this is not a .psu save. Other PS2 save formats (.max, .cbs, .sps, .xps, \
                 .psv) are not supported"
                    .into(),
            )
        })?;
    let before = inspect_memory_card_bytes(card, card_bytes);
    let with_ecc = before
        .ps2_geometry
        .as_ref()
        .is_some_and(|geometry| geometry.representation == Ps2PageRepresentation::RawWithSpare);
    if with_ecc {
        let (matching, written) = ecc_agreement(card_bytes);
        if matching != written {
            return Err(CardError::Refused(format!(
                "EmuWiz's error-correction codes do not reproduce the ones on this card \
                 ({} of {written} written pages differ), so it will not write a copy of it",
                written - matching
            )));
        }
    }
    let data_only = if with_ecc {
        strip_spare(card_bytes)?
    } else {
        card_bytes.to_vec()
    };

    // The existing planner works from files, so it gets a private working
    // copy of the card and of the save beside the new card.
    let work = working_folder(new_card)?;
    let working_card = work.path().join("card.ps2");
    write_new_file(&working_card, &data_only)?;
    let working_save = work.path().join("save.psu");
    write_new_file(&working_save, &write_psu_standard(&incoming))?;
    let working = inspect_memory_card(&working_card).map_err(CardError::Io)?;
    let plan = plan_ps2_psu_restore(
        &working,
        &working_save,
        &work.path().join("unused-backup"),
        false,
    )
    .map_err(|error| CardError::Refused(error.to_string()))?;
    let restored = build_restored_card(&plan, &data_only)
        .map_err(|error| CardError::Refused(error.to_string()))?;
    if restored.len() != data_only.len() {
        return Err(CardError::VerificationFailed(
            "the card would change size".into(),
        ));
    }

    let mut changed_pages = 0usize;
    let mut marker = card_bytes
        .get(PCSX2_MARKER_PAGE * RAW_PAGE..PCSX2_MARKER_PAGE * RAW_PAGE + PCSX2_MARKER_BYTES)
        .and_then(|bytes| bytes.try_into().ok())
        .map_or(0, u64::from_le_bytes);
    let image = if with_ecc {
        let mut image = Vec::with_capacity(card_bytes.len());
        for (index, original_page) in card_bytes.chunks_exact(RAW_PAGE).enumerate() {
            let new_data = &restored[index * PAGE..(index + 1) * PAGE];
            if new_data == &original_page[..PAGE] {
                image.extend_from_slice(original_page);
            } else {
                changed_pages += 1;
                let mut spare = [0u8; SPARE];
                ps2_memcard::ecc::fill_spare(new_data, &mut spare);
                image.extend_from_slice(new_data);
                image.extend_from_slice(&spare);
                // PCSX2 folds every 8 bytes it writes into its marker; do the
                // same for the pages written here, so PCSX2 sees a card that
                // changed (and re-reads it after loading an older save state).
                let written = &image[image.len() - RAW_PAGE..];
                for word in written.chunks_exact(8) {
                    marker ^= u64::from_le_bytes(word.try_into().unwrap_or([0; 8]));
                }
            }
        }
        if changed_pages > 0 {
            let at = PCSX2_MARKER_PAGE * RAW_PAGE;
            image[at..at + PCSX2_MARKER_BYTES].copy_from_slice(&marker.to_le_bytes());
        }
        image
    } else {
        changed_pages = restored
            .chunks_exact(PAGE)
            .zip(data_only.chunks_exact(PAGE))
            .filter(|(new, old)| new != old)
            .count();
        restored
    };

    // The new card, read by both readers.
    let after = inspect_memory_card_bytes(new_card, &image);
    if after.health != MemoryCardHealth::Healthy {
        return Err(CardError::VerificationFailed(
            "EmuWiz's own reader does not accept the new card".into(),
        ));
    }
    let library_before = ps2_memcard::MemoryCard::parse(card_bytes)
        .and_then(|card| card.saves())
        .map_err(|error| CardError::Refused(error.to_string()))?;
    let library_after = ps2_memcard::MemoryCard::parse(&image)
        .and_then(|card| card.saves())
        .map_err(|error| CardError::VerificationFailed(error.to_string()))?;
    for save in &library_before {
        let survived = library_after.iter().any(|candidate| {
            candidate.name == save.name
                && candidate.files.len() == save.files.len()
                && candidate
                    .files
                    .iter()
                    .zip(&save.files)
                    .all(|(new, old)| new.name == old.name && new.data == old.data)
        });
        if !survived {
            return Err(CardError::VerificationFailed(format!(
                "the save {:?} would not survive the import unchanged",
                save.name
            )));
        }
    }
    let name = plan.save_display_name.clone();
    // The second reader must find the imported files with exactly the bytes
    // the .psu holds.
    let mut landed: Vec<(Vec<u8>, Vec<u8>)> = library_after
        .iter()
        .find(|save| save.name == name)
        .map(|save| {
            save.files
                .iter()
                .map(|file| (file.name.clone().into_bytes(), file.data.clone()))
                .collect()
        })
        .unwrap_or_default();
    landed.sort();
    if library_after.len() != library_before.len() + 1 || landed != psu_contents(&incoming) {
        return Err(CardError::VerificationFailed(
            "the imported save does not read back identical to the file".into(),
        ));
    }
    if library_listing(&library_after, &unopened_folders(&after)) != own_listing(&after) {
        return Err(CardError::VerificationFailed(
            "the two readers list different saves on the new card".into(),
        ));
    }
    // Reading the save out again with EmuWiz's own exporter gives the same files.
    let working_after = work.path().join("after.ps2");
    write_new_file(&working_after, &image)?;
    let exported = read_save(&working_after, &image, &name, work.path())?;
    if psu_contents(&exported) != psu_contents(&incoming) {
        return Err(CardError::VerificationFailed(
            "the save exported again from the new card differs from the imported file".into(),
        ));
    }
    let mut checks = vec![
        if with_ecc {
            format!(
                "{changed_pages} page(s) of the card changed, plus PCSX2's 8-byte change \
                 marker; every other page is identical"
            )
        } else {
            format!("{changed_pages} page(s) of the card changed; every other page is identical")
        },
        "every save that was on the card is still there, every file byte for byte".into(),
        "the imported save's files read back identical to the .psu (two independent readers)"
            .into(),
    ];
    if with_ecc {
        let (matching, written) = ecc_agreement(&image);
        if matching != written {
            return Err(CardError::VerificationFailed(
                "the new card's error-correction codes do not check out".into(),
            ));
        }
        checks.push(format!(
            "all {written} written pages carry a valid error-correction code"
        ));
    }
    Ok(ImportOutcome {
        image,
        format: if with_ecc {
            CardFormat::Ps2WithEcc
        } else {
            CardFormat::Ps2DataOnly
        },
        save_name: name,
        checks,
    })
}
