//! PS1 cards. Containers, block chains and single-save files are read by the
//! `ps1-memcard` crate; EmuWiz's own reader is the cross-check. The only
//! writer here adds one save to free blocks of a *copy* of a card and leaves
//! every other byte of that copy as it was.

use super::{
    CardError, CardFormat, CardHealth, CardReport, CardSave, ImportOutcome, decode_title,
    product_code_and_region,
};
use crate::memory_card_inventory::{MemoryCardHealth, inspect_memory_card_bytes};
use ps1_memcard::{BLOCK, CAPACITY, FRAME, MemoryCard, SLOTS, Save, frame_checksum};
use std::path::Path;

const STATE_FIRST: u32 = 0x51;
const STATE_MIDDLE: u32 = 0x52;
const STATE_LAST: u32 = 0x53;
const STATE_FREE: u8 = 0xA0;
const NAME_OFFSET: usize = 0x0A;
const NAME_BYTES: usize = 20;

pub(super) fn detect(bytes: &[u8]) -> Option<CardFormat> {
    let inner = ps1_memcard::strip_container(bytes)?;
    if !inner.starts_with(b"MC") {
        return None;
    }
    match bytes.len() - CAPACITY {
        0 => Some(CardFormat::Ps1Raw),
        3904 => Some(CardFormat::Ps1DexDrive),
        64 => Some(CardFormat::Ps1Vgs),
        128 => Some(CardFormat::Ps1Vmp),
        _ => None,
    }
}

/// (slot, name, blocks) of every live save, as each reader sees it.
type Listing = Vec<(u32, String, u32)>;

fn library_listing(card: &MemoryCard) -> Listing {
    card.saves()
        .iter()
        .map(|save| {
            (
                save.slot as u32,
                save.name.clone(),
                (save.data.len() / BLOCK) as u32,
            )
        })
        .collect()
}

pub(super) fn report(path: &Path, bytes: &[u8], format: CardFormat, sha256: String) -> CardReport {
    let inner = ps1_memcard::strip_container(bytes).unwrap_or(bytes);
    let own = inspect_memory_card_bytes(path, inner);
    let library = MemoryCard::parse(bytes);
    let mut notes = Vec::new();
    let mut health = CardHealth::Healthy;

    if own.health != MemoryCardHealth::Healthy {
        health = CardHealth::Warnings;
        notes.extend(own.warnings.iter().cloned());
        for entry in &own.entries {
            notes.extend(
                entry
                    .warnings
                    .iter()
                    .filter(|_| !entry.deleted)
                    .map(|warning| format!("{}: {warning}", entry.name)),
            );
        }
    }
    let own_listing: Listing = own
        .entries
        .iter()
        .filter(|entry| !entry.deleted)
        .map(|entry| {
            (
                entry.slot.unwrap_or(0),
                entry.name.clone(),
                entry.used_blocks.unwrap_or(0),
            )
        })
        .collect();
    let readers_agree = match &library {
        Ok(card) => {
            let agree = library_listing(card) == own_listing;
            if !agree {
                health = CardHealth::Warnings;
                notes.push(
                    "EmuWiz's reader and the ps1-memcard reader list different saves on this \
                     card, so nothing is exported from or imported into it."
                        .into(),
                );
            }
            Some(agree)
        }
        Err(error) => {
            health = CardHealth::Warnings;
            notes.push(format!(
                "The ps1-memcard reader refused this card: {error}."
            ));
            Some(false)
        }
    };

    let saves = own_listing
        .iter()
        .map(|(slot, name, blocks)| {
            let block = *slot as usize * BLOCK;
            let title = inner.get(block + 4..block + 4 + 64).and_then(decode_title);
            let (product_code, region) = product_code_and_region(name);
            CardSave {
                name: name.clone(),
                product_code,
                region,
                title,
                size_bytes: u64::from(*blocks) * BLOCK as u64,
                blocks: Some(*blocks),
                file_count: None,
                modified: None,
                notes: Vec::new(),
            }
        })
        .collect::<Vec<_>>();

    let deleted = own.entries.iter().filter(|entry| entry.deleted).count();
    if deleted > 0 {
        notes.push(format!(
            "{deleted} deleted save(s) are still marked on the card. EmuWiz does not recover \
             them and does not reuse their blocks."
        ));
    }
    let mut unsupported = Vec::new();
    match format {
        CardFormat::Ps1Vmp => unsupported.push(
            "PSP/PS3 .vmp cards are signed. EmuWiz reads them and exports their saves, but \
             never writes one: importing produces a plain 128 KB card instead."
                .into(),
        ),
        CardFormat::Ps1DexDrive | CardFormat::Ps1Vgs => unsupported.push(
            "EmuWiz does not write DexDrive or VGS headers: importing produces a plain 128 KB \
             card (.mcd) instead."
                .into(),
        ),
        _ => {}
    }
    unsupported.push(
        "Single saves other than .mcs and PS1 .psv (.psx, .mcb, .mcx, .pda, raw blocks) cannot \
         be imported."
            .into(),
    );
    let healthy = health == CardHealth::Healthy;
    CardReport {
        path: path.to_path_buf(),
        format,
        size_bytes: bytes.len() as u64,
        sha256: Some(sha256),
        health,
        saves,
        free_space: own
            .free_blocks
            .map(|free| format!("{free} of {SLOTS} blocks free")),
        readers_agree,
        export_format: healthy.then_some("mcs"),
        import_formats: if healthy {
            vec!["mcs", "psv"]
        } else {
            Vec::new()
        },
        notes,
        unsupported,
    }
}

/// One save as a `.mcs` file: its 128-byte directory frame, then its blocks.
pub(super) fn export_mcs(card_bytes: &[u8], save_name: &str) -> Result<Vec<u8>, CardError> {
    let card =
        MemoryCard::parse(card_bytes).map_err(|error| CardError::Refused(error.to_string()))?;
    let save = card
        .saves()
        .iter()
        .find(|save| save.name == save_name)
        .ok_or_else(|| CardError::SaveNotFound(save_name.to_string()))?;
    let mut mcs = save.dirent.clone();
    mcs.extend_from_slice(&save.data);
    // The file must read back as the same save before it is offered.
    match ps1_memcard::read_single(&mcs) {
        Some(back) if back.name == save.name && back.data == save.data => Ok(mcs),
        _ => Err(CardError::VerificationFailed(
            "the exported save does not read back as the save on the card".into(),
        )),
    }
}

fn read_incoming(save_bytes: &[u8]) -> Result<Save, CardError> {
    let save = ps1_memcard::read_single(save_bytes).ok_or_else(|| {
        CardError::Unsupported(
            "this is not a .mcs or PS1 .psv save. Other single-save formats (.psx, .mcb, .mcx, \
             .pda, raw blocks) are not supported"
                .into(),
        )
    })?;
    let blocks = save.data.len() / BLOCK;
    if save.data.is_empty() || !save.data.len().is_multiple_of(BLOCK) || blocks > SLOTS {
        return Err(CardError::Refused(
            "the save is not a whole number of 8 KB blocks that fits a card".into(),
        ));
    }
    if !save.data.starts_with(b"SC") {
        return Err(CardError::Refused(
            "the save's first block does not start with the save header `SC`".into(),
        ));
    }
    if save.name.is_empty() || save.name.len() > NAME_BYTES || !save.name.is_ascii() {
        return Err(CardError::Refused(
            "the save has no usable name (1 to 20 ASCII characters)".into(),
        ));
    }
    Ok(save)
}

/// A copy of the card with one save added to its free blocks.
///
/// Only the blocks the save lands in and their directory frames differ from
/// the original; that is checked, along with both readers' view of the result.
pub(super) fn import_into_copy(
    card_path: &Path,
    card_bytes: &[u8],
    save_bytes: &[u8],
) -> Result<ImportOutcome, CardError> {
    let incoming = read_incoming(save_bytes)?;
    let original = ps1_memcard::strip_container(card_bytes)
        .ok_or_else(|| CardError::NotACard("not a PS1 card".into()))?;
    let before =
        MemoryCard::parse(original).map_err(|error| CardError::Refused(error.to_string()))?;
    if before.saves().iter().any(|save| save.name == incoming.name) {
        return Err(CardError::Refused(format!(
            "the card already has a save named {:?}; replacing a save is not supported",
            incoming.name
        )));
    }
    let needed = incoming.data.len() / BLOCK;
    let free: Vec<usize> = (1..=SLOTS)
        .filter(|slot| original[slot * FRAME] == STATE_FREE)
        .collect();
    if free.len() < needed {
        return Err(CardError::Refused(format!(
            "the save needs {needed} free block(s) and the card has {} (blocks of deleted \
             saves are not reused)",
            free.len()
        )));
    }
    let slots = &free[..needed];
    let mut image = original.to_vec();
    for (index, &slot) in slots.iter().enumerate() {
        image[slot * BLOCK..(slot + 1) * BLOCK]
            .copy_from_slice(&incoming.data[index * BLOCK..(index + 1) * BLOCK]);
        let mut frame = [0u8; FRAME];
        if index == 0 {
            if incoming.dirent.len() == FRAME {
                frame.copy_from_slice(&incoming.dirent);
            }
            frame[NAME_OFFSET..NAME_OFFSET + NAME_BYTES].fill(0);
            frame[NAME_OFFSET..NAME_OFFSET + incoming.name.len()]
                .copy_from_slice(incoming.name.as_bytes());
        }
        let state = match index {
            0 => STATE_FIRST,
            last if last + 1 == needed => STATE_LAST,
            _ => STATE_MIDDLE,
        };
        frame[..4].copy_from_slice(&state.to_le_bytes());
        // The length belongs to the first frame only; a console writes zero
        // on the continuation frames.
        let size = if index == 0 {
            (needed * BLOCK) as u32
        } else {
            0
        };
        frame[4..8].copy_from_slice(&size.to_le_bytes());
        // The link counts data blocks from zero, so block 1 is written as 0.
        let link = match slots.get(index + 1) {
            Some(next) => (*next - 1) as u16,
            None => 0xFFFF,
        };
        frame[8..10].copy_from_slice(&link.to_le_bytes());
        frame[FRAME - 1] = frame_checksum(&frame);
        image[slot * FRAME..(slot + 1) * FRAME].copy_from_slice(&frame);
    }

    // 1. Nothing outside the new save's blocks and frames changed.
    for (offset, (old, new)) in original.iter().zip(&image).enumerate() {
        if old != new {
            let in_frame = slots
                .iter()
                .any(|slot| (slot * FRAME..(slot + 1) * FRAME).contains(&offset));
            let in_block = slots
                .iter()
                .any(|slot| (slot * BLOCK..(slot + 1) * BLOCK).contains(&offset));
            if !in_frame && !in_block {
                return Err(CardError::VerificationFailed(format!(
                    "byte {offset} outside the imported save would change"
                )));
            }
        }
    }
    // 2. The library reads the result: every old save identical, the new one
    //    byte for byte the file's.
    let after = MemoryCard::parse(&image)
        .map_err(|error| CardError::VerificationFailed(error.to_string()))?;
    for save in before.saves() {
        if !after.saves().contains(save) {
            return Err(CardError::VerificationFailed(format!(
                "the save {:?} would not survive the import unchanged",
                save.name
            )));
        }
    }
    let imported = after.saves().iter().find(|save| save.name == incoming.name);
    if after.saves().len() != before.saves().len() + 1
        || imported.is_none_or(|save| save.data != incoming.data)
    {
        return Err(CardError::VerificationFailed(
            "the imported save does not read back identical to the file".into(),
        ));
    }
    // 3. EmuWiz's own reader agrees and finds the card healthy.
    let own = inspect_memory_card_bytes(card_path, &image);
    let live = own.entries.iter().filter(|entry| !entry.deleted).count();
    if own.health != MemoryCardHealth::Healthy || live != after.saves().len() {
        return Err(CardError::VerificationFailed(
            "EmuWiz's own reader does not accept the new card".into(),
        ));
    }
    Ok(ImportOutcome {
        image,
        format: CardFormat::Ps1Raw,
        save_name: incoming.name,
        checks: vec![
            format!(
                "only the {needed} block(s) of the new save and their directory entries differ \
                 from the original card"
            ),
            "every save that was on the card is still there, byte for byte".into(),
            "the imported save reads back identical to the file (two independent readers)".into(),
        ],
    })
}
