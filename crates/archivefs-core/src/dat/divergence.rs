//! Structured, non-destructive report comparing a DAT's own parsed
//! metadata against its filename.
//!
//! A DAT file's name on disk is presentation, exactly like an archive's
//! local filename is presentation elsewhere in this crate - it is never
//! trusted as identity or classification evidence. This module never
//! reads a filename in order to *decide* anything; it only compares a
//! filename's plain-text hints against facts this crate has already
//! parsed, and reports where the two disagree in a way a person would
//! want to know about. Parsed metadata is never overwritten, and a
//! filename never reclassifies a DAT's ecosystem, platform, or content.
//!
//! # Reuse, not a second parser
//!
//! Every comparison here reuses an existing, reviewed extractor rather
//! than inventing new text-matching rules:
//!
//! - platform/system: [`crate::dat::identity::gather_dat_platform_evidence`]
//!   already gathers both header-derived (`Strong`) and filename-derived
//!   (`Weak`, never decisive) platform evidence; this module simply
//!   compares the two tiers and reports when they disagree.
//! - region/language: [`crate::dat::policy::tags::regions_of_name`] /
//!   [`languages_of_name`](crate::dat::policy::tags::languages_of_name),
//!   the same strict tag extractors the matching policy ranks candidates
//!   with, applied to the DAT's own header text and to its filename.
//!
//! # Only decisive, two-sided disagreements are reported
//!
//! A field is reported only when *both* the metadata side and the
//! filename side carry actual evidence and that evidence disagrees.
//! Absence of evidence on either side is "unknown", never a mismatch -
//! this is what keeps an ordinary DAT whose header simply does not
//! mention region/language/version quiet, and it is why punctuation,
//! spacing, and case alone (all normalised away by the reused
//! extractors, or by exact substring matching after lower-casing) never
//! create a report.

use std::collections::BTreeSet;
use std::path::Path;

use crate::platform::equivalent_platform_ids;

use super::identity::{
    DatPlatformEvidenceKind, DatPlatformIdentity, gather_dat_platform_evidence,
    resolve_dat_platform_identity,
};
use super::model::{DatEcosystem, ParsedDat};
use super::policy::tags::{languages_of_name, regions_of_name};

/// Which reviewed field one [`FieldDivergence`] is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DivergenceField {
    Ecosystem,
    SystemPlatform,
    Region,
    Language,
    RevisionOrVersion,
    BiosFirmware,
    AftermarketHomebrew,
    ParentCloneRelationship,
}

impl DivergenceField {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ecosystem => "Catalogue/provider ecosystem",
            Self::SystemPlatform => "System/platform",
            Self::Region => "Region",
            Self::Language => "Language",
            Self::RevisionOrVersion => "Revision/version",
            Self::BiosFirmware => "BIOS/firmware status",
            Self::AftermarketHomebrew => "Aftermarket/homebrew status",
            Self::ParentCloneRelationship => "Parent/clone relationship",
        }
    }
}

/// One reviewed field where parsed metadata and the DAT filename suggest
/// materially different information. Both sides are exact evidence, never
/// a score: `metadata_evidence` names exactly what was parsed and from
/// where; `filename_hint` names exactly what the filename text suggests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDivergence {
    pub field: DivergenceField,
    pub metadata_evidence: String,
    pub filename_hint: String,
}

/// The complete, read-only result of comparing one DAT's metadata to its
/// own filename. Empty `divergences` means every reviewed field agreed
/// (or one/both sides had no evidence to compare) - never itself an
/// error or a warning.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DatMetadataFilenameReport {
    pub filename: String,
    pub divergences: Vec<FieldDivergence>,
}

/// Case-insensitive keyword hints a filename can carry for a provider
/// ecosystem. Corroboration only, exactly like
/// [`crate::dat::identity::DatPlatformEvidenceKind::FilenameCorroboration`] -
/// never used to set [`DatEcosystem`] anywhere in this crate.
const ECOSYSTEM_FILENAME_KEYWORDS: &[(&str, DatEcosystem)] = &[
    ("no-intro", DatEcosystem::NoIntro),
    ("redump", DatEcosystem::Redump),
    ("tosec", DatEcosystem::Tosec),
    ("fbneo", DatEcosystem::FBNeo),
    ("finalburn neo", DatEcosystem::FBNeo),
];

const BIOS_FILENAME_KEYWORDS: &[&str] = &["bios", "firmware"];
const AFTERMARKET_KEYWORDS: &[&str] = &["aftermarket", "homebrew", "unlicensed", "pirate"];
const CLONE_LIST_FILENAME_KEYWORDS: &[&str] = &["retool", "clone list", "1g1r", "parent-clone"];

/// Compares `dat`'s already-parsed metadata against its own filename
/// (`dat.source.file_path`'s basename) and reports every reviewed field
/// where the two suggest materially different information. Read-only:
/// never mutates `dat`, never touches the filesystem beyond the string
/// already carried in `file_path`.
pub fn compare_dat_metadata_to_filename(dat: &ParsedDat) -> DatMetadataFilenameReport {
    let filename = Path::new(&dat.source.file_path)
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| dat.source.file_path.clone());
    let stem = Path::new(&filename)
        .file_stem()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| filename.clone());
    let stem_lower = stem.to_ascii_lowercase();

    let mut divergences = Vec::new();

    // Ecosystem: a filename keyword is corroboration only, never authority.
    for (keyword, suggested) in ECOSYSTEM_FILENAME_KEYWORDS {
        if stem_lower.contains(keyword) && *suggested != dat.source.ecosystem {
            divergences.push(FieldDivergence {
                field: DivergenceField::Ecosystem,
                metadata_evidence: format!(
                    "parsed DAT ecosystem is {}",
                    dat.source.ecosystem.label()
                ),
                filename_hint: format!(
                    "filename contains \"{keyword}\", suggesting {}",
                    suggested.label()
                ),
            });
        }
    }

    // System/platform: reuse the existing evidence gathering wholesale -
    // compare its own header-derived (Strong) resolution against its own
    // filename-derived (Weak) candidates, never a new text comparison.
    let all_evidence = gather_dat_platform_evidence(dat);
    let header_identity =
        resolve_dat_platform_identity(all_evidence.iter().cloned().filter(|evidence| {
            !matches!(
                evidence.kind,
                DatPlatformEvidenceKind::FilenameCorroboration
                    | DatPlatformEvidenceKind::FolderHint
                    | DatPlatformEvidenceKind::MediaExtension
            )
        }));
    if let DatPlatformIdentity::Resolved {
        platform: header_platform,
        ..
    } = &header_identity
    {
        let equivalents = equivalent_platform_ids(header_platform);
        let mut filename_candidates: BTreeSet<&str> = all_evidence
            .iter()
            .filter(|evidence| evidence.kind == DatPlatformEvidenceKind::FilenameCorroboration)
            .map(|evidence| evidence.platform.as_str())
            .collect();
        filename_candidates.retain(|candidate| {
            *candidate != header_platform.as_str() && !equivalents.contains(candidate)
        });
        for candidate in filename_candidates {
            divergences.push(FieldDivergence {
                field: DivergenceField::SystemPlatform,
                metadata_evidence: format!("DAT header identifies platform \"{header_platform}\""),
                filename_hint: format!("filename suggests platform \"{candidate}\""),
            });
        }
    }

    // Region / language: the same strict tag extractors the matching
    // policy ranks on, applied to the DAT's own header text versus its
    // filename. Only a two-sided disagreement (both non-empty, and
    // different) is reported - absence of evidence on either side is
    // unknown, never a mismatch.
    let header_text = dat
        .source
        .name
        .clone()
        .or_else(|| dat.source.description.clone());
    if let Some(header_text) = &header_text {
        let header_regions: BTreeSet<_> = regions_of_name(header_text).into_iter().collect();
        let filename_regions: BTreeSet<_> = regions_of_name(&stem).into_iter().collect();
        if !header_regions.is_empty()
            && !filename_regions.is_empty()
            && header_regions != filename_regions
        {
            divergences.push(FieldDivergence {
                field: DivergenceField::Region,
                metadata_evidence: format!("DAT header text names region(s) {header_regions:?}"),
                filename_hint: format!("filename names region(s) {filename_regions:?}"),
            });
        }

        let header_languages: BTreeSet<_> = languages_of_name(header_text).into_iter().collect();
        let filename_languages: BTreeSet<_> = languages_of_name(&stem).into_iter().collect();
        if !header_languages.is_empty()
            && !filename_languages.is_empty()
            && header_languages != filename_languages
        {
            divergences.push(FieldDivergence {
                field: DivergenceField::Language,
                metadata_evidence: format!(
                    "DAT header text names language(s) {header_languages:?}"
                ),
                filename_hint: format!("filename names language(s) {filename_languages:?}"),
            });
        }
    }

    // Revision/version: the DAT's own declared header version against a
    // date/version-shaped digit run embedded in the filename. Both sides
    // must carry a real digit run; anything shorter is too ambiguous
    // (page numbers, single digits in a title) to treat as a version.
    if let Some(version) = dat.source.version.as_deref() {
        if let (Some(version_digits), Some(filename_digits)) =
            (longest_digit_run(version), longest_digit_run(&stem))
            && version_digits != filename_digits
        {
            divergences.push(FieldDivergence {
                field: DivergenceField::RevisionOrVersion,
                metadata_evidence: format!("DAT header declares version \"{version}\""),
                filename_hint: format!(
                    "filename carries the date/version digits \"{filename_digits}\""
                ),
            });
        }
    }

    // BIOS/firmware: whether the catalogue actually declares any BIOS
    // entry versus whether the filename claims one.
    let any_bios_entry = dat.games.iter().any(|game| {
        game.is_bios
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("yes"))
    });
    let filename_claims_bios = BIOS_FILENAME_KEYWORDS
        .iter()
        .any(|keyword| stem_lower.contains(keyword));
    if any_bios_entry != filename_claims_bios {
        divergences.push(FieldDivergence {
            field: DivergenceField::BiosFirmware,
            metadata_evidence: if any_bios_entry {
                "catalogue declares at least one BIOS entry".to_string()
            } else {
                "catalogue declares no BIOS entry".to_string()
            },
            filename_hint: if filename_claims_bios {
                "filename suggests a BIOS/firmware set".to_string()
            } else {
                "filename does not suggest a BIOS/firmware set".to_string()
            },
        });
    }

    // Aftermarket/homebrew: no structured field exists for this yet (see
    // module doc comment on `DatContentClass`), so both sides are read
    // from text this crate already treats as metadata: the DAT's own
    // header name/description versus the filename.
    let header_claims_aftermarket = header_text
        .as_deref()
        .map(str::to_ascii_lowercase)
        .is_some_and(|text| {
            AFTERMARKET_KEYWORDS
                .iter()
                .any(|keyword| text.contains(keyword))
        });
    let filename_claims_aftermarket = AFTERMARKET_KEYWORDS
        .iter()
        .any(|keyword| stem_lower.contains(keyword));
    if header_claims_aftermarket != filename_claims_aftermarket {
        divergences.push(FieldDivergence {
            field: DivergenceField::AftermarketHomebrew,
            metadata_evidence: if header_claims_aftermarket {
                "DAT header text names an aftermarket/homebrew set".to_string()
            } else {
                "DAT header text does not name an aftermarket/homebrew set".to_string()
            },
            filename_hint: if filename_claims_aftermarket {
                "filename suggests an aftermarket/homebrew set".to_string()
            } else {
                "filename does not suggest an aftermarket/homebrew set".to_string()
            },
        });
    }

    // Parent/clone relationship: only the actionable direction is
    // reported - a filename that implies a curated clone-aware list (a
    // Retool-style export, a "1G1R" pack) but whose parsed content
    // declares no parent/clone relationship at all. The reverse (an
    // ordinary catalogue with real `cloneof` data and a filename that
    // simply does not say "Retool") is completely normal and would be
    // pure noise.
    let any_clone_relationship = dat.games.iter().any(|game| {
        game.clone_of
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
    });
    let filename_claims_clone_list = CLONE_LIST_FILENAME_KEYWORDS
        .iter()
        .any(|keyword| stem_lower.contains(keyword));
    if filename_claims_clone_list && !any_clone_relationship {
        divergences.push(FieldDivergence {
            field: DivergenceField::ParentCloneRelationship,
            metadata_evidence: "catalogue declares no parent/clone (cloneof) relationship at all"
                .to_string(),
            filename_hint: "filename suggests a curated parent/clone (Retool-style) list"
                .to_string(),
        });
    }

    DatMetadataFilenameReport {
        filename,
        divergences,
    }
}

/// The longest contiguous run of ASCII digits in `text`, at least 6 digits
/// long (short enough to be a page number or a single-digit revision, not
/// a date/version stamp). Ties keep the first (leftmost) run.
fn longest_digit_run(text: &str) -> Option<String> {
    let mut best: Option<String> = None;
    let mut current = String::new();
    let flush = |current: &mut String, best: &mut Option<String>| {
        if current.len() >= 6
            && best
                .as_ref()
                .is_none_or(|found| current.len() > found.len())
        {
            *best = Some(current.clone());
        }
        current.clear();
    };
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            current.push(ch);
        } else {
            flush(&mut current, &mut best);
        }
    }
    flush(&mut current, &mut best);
    best
}

#[cfg(test)]
mod tests;
