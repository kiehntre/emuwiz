//! Can this save be continued somewhere else?
//!
//! Five answers, and the rule that produced each one. "Verified" and
//! "after a verified conversion" are only given where EmuWiz has code that
//! performs the step and checks the result byte for byte; everything else is
//! conditional, incompatible or unknown, and says why.
//!
//! The one conversion implemented is the PlayStation memory-card route of
//! [`crate::memory_card_manager`]: one save is exported to a single-save file
//! and imported into a **copy** of the destination card. Both cards are only
//! read.

use super::model::{AssociationStrength, DiscoveredSave, SaveFormat};
use super::{ContinuityError, sha256_hex};
use crate::memory_card_manager::{
    CardHealth, CardPlatform, export_save, import_save_into_copy, read_card_with_fingerprints,
};
use serde::Serialize;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Compatibility {
    /// The destination reads this exact format; nothing needs converting.
    Verified,
    /// Works after a conversion EmuWiz performs and checks.
    AfterVerifiedConversion,
    /// The format matches, but conditions EmuWiz has not checked must hold.
    Conditional,
    Incompatible,
    /// EmuWiz has no evidence either way.
    Unknown,
}

impl Compatibility {
    pub fn label(self) -> &'static str {
        match self {
            Self::Verified => "Verified compatible",
            Self::AfterVerifiedConversion => "Compatible after a checked conversion",
            Self::Conditional => "Compatible only if the listed conditions hold",
            Self::Incompatible => "Not compatible",
            Self::Unknown => "Not known",
        }
    }
}

/// Where the person wants to continue.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContinuationTarget {
    pub emulator: String,
    /// `native` / `flatpak`, when it matters.
    pub profile: String,
    /// libretro core, for a RetroArch target.
    pub core: Option<String>,
    pub emulator_version: Option<String>,
    /// The product code of the game as it will be run at the destination,
    /// when known (from a verified game file). `None` = not established.
    pub game_product_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConversionPlan {
    pub source_format: String,
    pub destination_format: String,
    pub steps: Vec<String>,
    pub conditions: Vec<String>,
    pub risks: Vec<String>,
    pub validation: Vec<String>,
    /// EmuWiz can carry this out today ([`prepare_card_continuation`]).
    pub implemented: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Assessment {
    pub verdict: Compatibility,
    pub summary: String,
    pub reasons: Vec<String>,
    /// Things a person must confirm; EmuWiz has not checked them.
    pub unchecked_conditions: Vec<String>,
    pub conversion: Option<ConversionPlan>,
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn card_conversion(platform: &str, single: &str, save: &DiscoveredSave) -> ConversionPlan {
    ConversionPlan {
        source_format: format!("one save on a {platform} memory card ({})", save.source_path.display()),
        destination_format: format!("the same save on a copy of the destination {platform} card"),
        steps: vec![
            format!("export the save from the source card to a new {single} file"),
            "make a new card: a copy of the destination card with that save added".to_string(),
            "leave both original cards exactly as they are".to_string(),
        ],
        conditions: vec![
            "both cards read cleanly with two independent readers".to_string(),
            "the destination card has room and no save of the same name".to_string(),
            "the game at the destination is the same release (same product code)".to_string(),
        ],
        risks: vec![
            "the game may keep related data in a second save (a system or profile save) that must travel with it".to_string(),
            "the new card is not used by the emulator until a person puts it in place".to_string(),
        ],
        validation: vec![
            "the new card is read back by both readers".to_string(),
            "the imported save has the same SHA-256 as on the source card".to_string(),
            "every save already on the destination card is still there, byte for byte".to_string(),
            "both original cards have the same SHA-256 before and after".to_string(),
        ],
        implemented: true,
    }
}

/// Assesses one save against one destination. Reads nothing; pure rules.
pub fn assess(save: &DiscoveredSave, target: &ContinuationTarget) -> Assessment {
    let mut reasons = Vec::new();
    let mut unchecked = Vec::new();
    let done = |verdict, summary: &str, reasons, unchecked, conversion| Assessment {
        verdict,
        summary: summary.to_string(),
        reasons,
        unchecked_conditions: unchecked,
        conversion,
    };

    if save.identity.strength < AssociationStrength::ContentProven {
        reasons.push(format!(
            "which game this save belongs to is not confirmed: {}",
            save.identity.strength.label()
        ));
    }
    // A different release looks for a different product code.
    if let (Some(have), Some(want)) = (&save.identity.product_code, &target.game_product_code) {
        if !same(have, want) {
            reasons.push(format!(
                "the save was written by {have}; the game at the destination is {want}. A different release does not look for this save"
            ));
            return done(
                Compatibility::Incompatible,
                "Different release of the game",
                reasons,
                unchecked,
                None,
            );
        }
        reasons.push(format!("same release at both ends ({have})"));
    } else if save.identity.product_code.is_some() {
        unchecked.push(
            "the game at the destination is the same release (same product code) as the save"
                .to_string(),
        );
    }

    let same_place = same(&save.emulator, &target.emulator) && same(&save.profile, &target.profile);
    match save.format {
        SaveFormat::Ps1CardSave | SaveFormat::Ps2CardSave => {
            let ps2 = save.format == SaveFormat::Ps2CardSave;
            let (platform, single) = if ps2 {
                ("PlayStation 2", ".psu")
            } else {
                ("PlayStation", ".mcs")
            };
            if same_place {
                reasons.push("the save is already in this emulator's own card".to_string());
                return done(
                    Compatibility::Verified,
                    "Already where this emulator looks",
                    reasons,
                    unchecked,
                    None,
                );
            }
            let reads_cards = if ps2 {
                same(&target.emulator, "PCSX2")
            } else {
                same(&target.emulator, "DuckStation") || same(&target.emulator, "PCSX2")
            };
            if reads_cards {
                if save
                    .unsupported
                    .iter()
                    .any(|line| line == super::discover::CARD_HAS_WARNINGS)
                {
                    reasons.push(super::discover::CARD_HAS_WARNINGS.to_string());
                    return done(
                        Compatibility::Conditional,
                        "The source card must read cleanly first",
                        reasons,
                        unchecked,
                        None,
                    );
                }
                reasons.push(format!(
                    "{} keeps {platform} saves on the same kind of memory card",
                    target.emulator
                ));
                return done(
                    Compatibility::AfterVerifiedConversion,
                    "Can be copied onto a new card for the destination",
                    reasons,
                    unchecked,
                    Some(card_conversion(platform, single, save)),
                );
            }
            if same(&target.emulator, "RetroArch") {
                reasons.push(format!(
                    "RetroArch's {platform} cores use memory-card images, but EmuWiz has not checked which card file a given core opens or its naming"
                ));
                unchecked.push(
                    "the core in use reads a standard card image from the place you put it"
                        .to_string(),
                );
                return done(
                    Compatibility::Conditional,
                    "Same card format; placement not checked",
                    reasons,
                    unchecked,
                    None,
                );
            }
            if !ps2 && same(&target.emulator, "RPCS3") {
                reasons.push("RPCS3 keeps PS1 saves in signed .vmp cards; EmuWiz cannot produce a valid signature".to_string());
                return done(
                    Compatibility::Incompatible,
                    "Needs a signed card EmuWiz cannot make",
                    reasons,
                    unchecked,
                    None,
                );
            }
            reasons.push(format!(
                "{} does not use {platform} memory cards",
                target.emulator
            ));
            done(
                Compatibility::Incompatible,
                "The destination does not read this kind of save",
                reasons,
                unchecked,
                None,
            )
        }
        SaveFormat::GameCubeGci => {
            if !same(&target.emulator, "Dolphin") && !same(&target.emulator, "RetroArch") {
                reasons.push(format!("{} does not run GameCube games", target.emulator));
                return done(
                    Compatibility::Incompatible,
                    "The destination does not read this kind of save",
                    reasons,
                    unchecked,
                    None,
                );
            }
            if same_place {
                return done(
                    Compatibility::Verified,
                    "Already where this emulator looks",
                    reasons,
                    unchecked,
                    None,
                );
            }
            reasons.push(
                "a .gci file is the same on any Dolphin; EmuWiz has not tested a copy end to end"
                    .to_string(),
            );
            unchecked.push(
                "the destination uses a GCI folder (not a raw card image) for this slot"
                    .to_string(),
            );
            unchecked.push("the file goes into the folder for the game's region".to_string());
            done(
                Compatibility::Conditional,
                "Same file format; placement not checked",
                reasons,
                unchecked,
                None,
            )
        }
        SaveFormat::Ps3Savedata | SaveFormat::PspSavedata => {
            let home = if save.format == SaveFormat::Ps3Savedata {
                "RPCS3"
            } else {
                "PPSSPP"
            };
            if !same(&target.emulator, home) {
                reasons.push(format!("only {home} reads this kind of save folder"));
                return done(
                    Compatibility::Incompatible,
                    "The destination does not read this kind of save",
                    reasons,
                    unchecked,
                    None,
                );
            }
            if same_place {
                return done(
                    Compatibility::Verified,
                    "Already where this emulator looks",
                    reasons,
                    unchecked,
                    None,
                );
            }
            reasons.push("the save folder is signed for a user and console identity; EmuWiz does not verify or redo that signature".to_string());
            unchecked.push(
                "the destination uses the same user id and accepts the save's signature"
                    .to_string(),
            );
            unchecked.push("the game version and installed updates match".to_string());
            done(
                Compatibility::Unknown,
                "Same emulator, but the save's signature is not checked",
                reasons,
                unchecked,
                None,
            )
        }
        SaveFormat::CartridgeBattery => {
            if !same(&target.emulator, "RetroArch") {
                reasons.push(format!(
                    "EmuWiz has no checked rule for how {} stores this system's battery saves",
                    target.emulator
                ));
                return done(
                    Compatibility::Unknown,
                    "No evidence for this destination",
                    reasons,
                    unchecked,
                    None,
                );
            }
            reasons.push("the file does not record which core wrote it, and cores for one system can lay the same save out differently (clock data, combined files)".to_string());
            unchecked.push("the destination core is the same core, or one documented to share this save layout".to_string());
            unchecked.push("the game file is the same revision".to_string());
            done(
                Compatibility::Conditional,
                "Depends on the core at the destination",
                reasons,
                unchecked,
                None,
            )
        }
        SaveFormat::SaveState => {
            if !same(&save.emulator, &target.emulator) {
                reasons.push(
                    "a save state is one emulator's internal memory; no other emulator reads it"
                        .to_string(),
                );
                return done(
                    Compatibility::Incompatible,
                    "Save states do not move between emulators",
                    reasons,
                    unchecked,
                    None,
                );
            }
            reasons.push(
                "which core and version wrote this state is not recorded where EmuWiz can read it"
                    .to_string(),
            );
            unchecked.push("exactly the same core and core version at the destination".to_string());
            done(
                Compatibility::Unknown,
                "Only the same core and version can load it; that is not established",
                reasons,
                unchecked,
                None,
            )
        }
    }
}

/// A continuation copy that was written and checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContinuationCopy {
    pub save_name: String,
    /// The single-save file exported from the source card.
    pub exported_save: PathBuf,
    /// The new card: the destination card plus the save.
    pub new_card: PathBuf,
    pub new_card_sha256: String,
    pub save_fingerprint: String,
    pub source_card_sha256: String,
    pub destination_card_sha256: String,
    pub checks: Vec<String>,
    /// What a person still has to do, and what EmuWiz did not test.
    pub next: Vec<String>,
}

/// The destination card already holds a save of this name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SaveConflict {
    pub save_name: String,
    pub source_fingerprint: String,
    pub destination_fingerprint: String,
    pub identical: bool,
    pub explanation: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContinuationError {
    /// Needs a person's decision; nothing was written.
    Conflict(SaveConflict),
    Stopped(ContinuityError),
}

impl std::fmt::Display for ContinuationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict(conflict) => f.write_str(&conflict.explanation),
            Self::Stopped(error) => error.fmt(f),
        }
    }
}

fn stopped(text: impl ToString) -> ContinuationError {
    ContinuationError::Stopped(ContinuityError::Refused(text.to_string()))
}

fn file_sha(path: &Path) -> Result<String, ContinuationError> {
    fs::read(path)
        .map(|bytes| sha256_hex(&bytes))
        .map_err(|error| ContinuationError::Stopped(super::io("cannot read", path, error)))
}

/// Builds a continuation copy for one PS1 or PS2 card save: a new card in
/// `work_dir` (a folder that must not exist yet) holding everything on
/// `destination_card` plus `save_name` from `source_card`.
///
/// Both cards are only read. If the destination already has a save of that
/// name, nothing is written and the two are reported for a decision.
pub fn prepare_card_continuation(
    source_card: &Path,
    save_name: &str,
    destination_card: &Path,
    work_dir: &Path,
) -> Result<ContinuationCopy, ContinuationError> {
    if !work_dir.is_absolute() {
        return Err(stopped("the working folder must be an absolute path"));
    }
    for card in [source_card, destination_card] {
        if card
            .parent()
            .is_some_and(|parent| work_dir.starts_with(parent))
        {
            return Err(stopped(
                "the working folder must be outside the emulators' own card folders",
            ));
        }
    }
    if fs::symlink_metadata(work_dir).is_ok() {
        return Err(stopped(format!(
            "{} already exists; a continuation copy goes into a new folder",
            work_dir.display()
        )));
    }
    let card_error = |error: crate::memory_card_manager::CardError| stopped(error);
    let (source, source_prints) = read_card_with_fingerprints(source_card).map_err(card_error)?;
    let (destination, destination_prints) =
        read_card_with_fingerprints(destination_card).map_err(card_error)?;
    if source.format.platform() != destination.format.platform()
        || source.format.platform().is_none()
    {
        return Err(stopped("the two cards are not for the same console"));
    }
    for (report, which) in [(&source, "source"), (&destination, "destination")] {
        if report.health != CardHealth::Healthy {
            return Err(stopped(format!(
                "the {which} card has warnings: {}",
                report.notes.join("; ")
            )));
        }
    }
    let source_print = source_prints.get(save_name).cloned().ok_or_else(|| {
        ContinuationError::Stopped(ContinuityError::NotFound(format!(
            "{save_name} on the source card"
        )))
    })?;
    if let Some(existing) = destination_prints.get(save_name) {
        let identical = *existing == source_print;
        return Err(ContinuationError::Conflict(SaveConflict {
            save_name: save_name.to_string(),
            source_fingerprint: source_print,
            destination_fingerprint: existing.clone(),
            identical,
            explanation: if identical {
                format!(
                    "the destination card already holds {save_name} with exactly the same content; nothing to do"
                )
            } else {
                format!(
                    "the destination card already holds a different {save_name}. EmuWiz cannot tell which has more progress \
                     (neither date nor size says so) and will not replace either. Nothing was written"
                )
            },
        }));
    }
    let (source_sha, destination_sha) = (
        source
            .sha256
            .clone()
            .ok_or_else(|| stopped("folder cards are not supported here"))?,
        destination
            .sha256
            .clone()
            .ok_or_else(|| stopped("folder cards are not supported here"))?,
    );
    fs::DirBuilder::new()
        .mode(0o700)
        .create(work_dir)
        .map_err(|error| ContinuationError::Stopped(super::io("cannot create", work_dir, error)))?;
    let result = (|| {
        let single = if source.format.platform() == Some(CardPlatform::PlayStation2) {
            "psu"
        } else {
            "mcs"
        };
        let safe: String = save_name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let exported = work_dir.join(format!("{safe}.{single}"));
        export_save(source_card, save_name, &exported).map_err(card_error)?;
        let card_name = destination_card
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let new_card = work_dir.join(&card_name);
        let import =
            import_save_into_copy(destination_card, &exported, &new_card).map_err(card_error)?;
        // Independent of the importer's own checks: read the result again.
        let (after, after_prints) = read_card_with_fingerprints(&new_card).map_err(card_error)?;
        if after.health != CardHealth::Healthy {
            return Err(stopped("the new card does not read cleanly"));
        }
        if after_prints.get(save_name) != Some(&source_print) {
            return Err(stopped(
                "the save on the new card is not identical to the save on the source card",
            ));
        }
        for (name, print) in &destination_prints {
            if after_prints.get(name) != Some(print) {
                return Err(stopped(format!(
                    "{name}, already on the destination card, did not survive unchanged"
                )));
            }
        }
        if after_prints.len() != destination_prints.len() + 1 {
            return Err(stopped(
                "the new card holds saves that were on neither card",
            ));
        }
        if file_sha(source_card)? != source_sha || file_sha(destination_card)? != destination_sha {
            return Err(stopped(
                "an original card changed while the copy was being made",
            ));
        }
        let mut checks = import.checks;
        checks.extend([
            format!("{save_name} on the new card has the same SHA-256 as on the source card"),
            format!(
                "all {} saves already on the destination card are unchanged on the new card",
                destination_prints.len()
            ),
            "the source and destination cards have the same SHA-256 as before".to_string(),
        ]);
        Ok(ContinuationCopy {
            save_name: save_name.to_string(),
            exported_save: exported,
            new_card,
            new_card_sha256: import.new_card_sha256,
            save_fingerprint: source_print.clone(),
            source_card_sha256: source_sha.clone(),
            destination_card_sha256: destination_sha.clone(),
            checks,
            next: vec![
                "Nothing the emulator uses was changed. To play from the new card, close the emulator and choose the new card file in its memory-card settings (or put it in place yourself, keeping the old card).".to_string(),
                "EmuWiz checked the card's structure and bytes. It did not start the game.".to_string(),
            ],
        })
    })();
    if result.is_err() {
        // Only the folder this call created.
        let _ = fs::remove_dir_all(work_dir);
    }
    result
}
