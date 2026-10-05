//! One canonical answer to "is this game identified, and if not, why?".
//!
//! Verification is automatic: a game whose own evidence resolves, or whose
//! exact cryptographic hash matches one entry of an authoritative DAT (an
//! audit already recorded it), is simply **Verified** - nobody confirms it.
//! This module never verifies anything. It only reads what the identity
//! machinery already decided and explains the games that did *not* verify.
//!
//! Release class (Beta, Prototype, Demo, ...) and dump quality (known bad,
//! fixed, cracked, ...) are separate from verification: a Beta with an exact
//! authoritative match is Verified and stays a Beta; a known bad dump is
//! identified but never called a clean preservation dump.
use super::library::{Game, IdentityContext, UNKNOWN_PLATFORM};
use archivefs_core::dat::library_identity_summary::{
    DatProvenanceFreshness, DatVerificationState, LibraryDatIdentitySummary,
};
use archivefs_core::identity_attention::{
    ChoiceReason, IdentityAttention, IdentityFacts, InformationalReason, classify_identity,
    is_special_release,
};

/// What kind of release the matched (or, for an unidentified file, the
/// apparent) release is. Never an authority on its own.
pub(super) fn release_class(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    let has = |tag: &str| lower.contains(&format!("({tag}"));
    [
        ("beta", "Beta"),
        ("proto", "Prototype"),
        ("demo", "Demo"),
        ("sample", "Sample"),
        ("preview", "Preview"),
        ("kiosk", "Kiosk / store demo"),
        ("promo", "Promotional"),
        ("aftermarket", "Aftermarket"),
        ("homebrew", "Homebrew"),
        ("hack", "Hack"),
        ("unl", "Unlicensed"),
    ]
    .into_iter()
    .find(|(tag, _)| has(tag))
    .map(|(_, label)| label)
    .or_else(|| {
        // GoodTools-style bracket tags and path-level markers still say "special".
        is_special_release(std::path::Path::new(name)).then_some("Special release")
    })
}

/// How clean the matched dump is, from the trusted DAT entry's own name tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DumpQuality {
    Clean,
    KnownBad,
    Overdump,
    Underdump,
    Fixed,
    Modified,
    Cracked,
    Trained,
    Hacked,
    Alternate,
}

impl DumpQuality {
    /// Read only from the DAT's canonical entry name (trusted metadata) -
    /// never from the file on disk.
    pub(super) fn from_dat_name(name: &str) -> Self {
        let lower = name.to_ascii_lowercase();
        if lower.contains("(bad dump)") || lower.contains("(bad)") {
            return Self::KnownBad;
        }
        let mut rest = lower.as_str();
        while let Some(start) = rest.find('[') {
            let Some(end) = rest[start..].find(']') else {
                break;
            };
            let tag = &rest[start + 1..start + end];
            let letters = tag.trim_end_matches(|ch: char| ch.is_ascii_digit());
            let found = match letters {
                "b" => Some(Self::KnownBad),
                "o" => Some(Self::Overdump),
                "u" => Some(Self::Underdump),
                "f" => Some(Self::Fixed),
                "m" => Some(Self::Modified),
                "cr" => Some(Self::Cracked),
                "t" => Some(Self::Trained),
                "h" => Some(Self::Hacked),
                "a" => Some(Self::Alternate),
                _ => None,
            };
            if let Some(found) = found {
                return found;
            }
            rest = &rest[start + end + 1..];
        }
        Self::Clean
    }

    pub(super) fn label(self) -> Option<&'static str> {
        match self {
            Self::Clean => None,
            Self::KnownBad => Some("Known bad dump"),
            Self::Overdump => Some("Overdump"),
            Self::Underdump => Some("Underdump"),
            Self::Fixed => Some("Fixed dump"),
            Self::Modified => Some("Modified dump"),
            Self::Cracked => Some("Cracked"),
            Self::Trained => Some("Trained"),
            Self::Hacked => Some("Hacked"),
            Self::Alternate => Some("Alternate dump"),
        }
    }
}

/// What a recorded DAT audit says about one game, trimmed for the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DatKnowledge {
    pub state: DatVerificationState,
    /// The source is a configured DAT adapter (No-Intro, Redump, TOSEC, MAME,
    /// ...). A research or website source has no adapter and is never one.
    pub trusted_source: bool,
    pub stale: bool,
    pub title: Option<String>,
    pub region: Option<String>,
    pub revision: Option<String>,
    pub source_name: String,
    pub ecosystem: Option<&'static str>,
    pub candidates: Vec<String>,
    /// CRC / SHA-1 / provenance, for the Technical details section only.
    pub technical: String,
}

impl DatKnowledge {
    pub(super) fn from_summary(summary: &LibraryDatIdentitySummary) -> Self {
        let technical = format!(
            "Source: {} ({})\nDAT file: {}\nCatalogue revision: {}\nMatched with: {} {}\nHashes held: {}\nAudit state: {:?}\nFreshness: {:?}",
            summary.source.source_name,
            summary.source.source_id,
            summary.source.dat_path,
            summary
                .source
                .source_revision
                .as_deref()
                .unwrap_or("not recorded"),
            summary
                .hash_evidence
                .matched_algorithm
                .as_deref()
                .unwrap_or("no hash"),
            summary.hash_evidence.matched_value.as_deref().unwrap_or(""),
            summary.hash_evidence.available_algorithms.join(", "),
            summary.verification_state,
            summary.provenance_freshness,
        );
        Self {
            state: summary.verification_state.clone(),
            trusted_source: summary.source.ecosystem.is_some(),
            stale: summary.provenance_freshness == DatProvenanceFreshness::Stale,
            title: summary.canonical.canonical_dat_name.clone(),
            region: summary.canonical.region.clone(),
            revision: summary.canonical.revision.clone(),
            source_name: summary.source.source_name.clone(),
            ecosystem: summary.source.ecosystem.map(|ecosystem| ecosystem.label()),
            candidates: summary.ambiguous_candidates.clone(),
            technical,
        }
    }

    fn rank(&self) -> u8 {
        match &self.state {
            DatVerificationState::VerifiedSingleMatch { .. } => 6,
            DatVerificationState::Conflicting { .. } => 5,
            DatVerificationState::AmbiguousMultipleCandidates { .. } => 4,
            DatVerificationState::Probable => 3,
            DatVerificationState::FilenameOnlyNotVerified => 2,
            DatVerificationState::NoMatch => 1,
            DatVerificationState::NoUsableEvidence => 0,
        }
    }

    /// The most decisive of several sources' answers for one game.
    pub(super) fn best(all: Vec<Self>) -> Option<Self> {
        all.into_iter()
            .filter(|knowledge| !knowledge.stale)
            .max_by_key(Self::rank)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NoMatchReason {
    /// The installed data was searched and the file's exact hash is not in it.
    NotInData,
    /// Only a CRC or a filename matched: not enough to verify.
    WeakEvidenceOnly,
    /// The file gave nothing to compare.
    NoUsableEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VerifiedFacts {
    pub title: String,
    pub region: Option<String>,
    pub release: Option<&'static str>,
    pub dump: DumpQuality,
    pub source: Option<String>,
    /// Verified by the file's own evidence rather than by a DAT audit.
    pub by_file_evidence: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ReviewState {
    Verified(VerifiedFacts),
    /// Matched to MAME/arcade reference data by a completed or partial set audit.
    ReferenceMatched,
    Conflict {
        detail: String,
    },
    Ambiguous {
        candidates: Vec<String>,
    },
    NoSystem,
    /// No usable identification data for this system is installed.
    NoData {
        reference_source_exists: bool,
    },
    /// Data may exist, but this game was never compared with it.
    NotCompared,
    NoMatch(NoMatchReason),
}

/// The one identity state of a game, plus the release class that explains
/// unresolved special releases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Review {
    pub state: ReviewState,
    /// For an unresolved file this is only what the file name *suggests*.
    pub apparent_release: Option<&'static str>,
}

impl Review {
    pub(super) fn is_verified(&self) -> bool {
        matches!(self.state, ReviewState::Verified(_))
    }

    /// A short word for rows and badges.
    pub(super) fn list_label(&self) -> &'static str {
        match &self.state {
            ReviewState::Verified(_) => "Verified",
            ReviewState::ReferenceMatched => "Matched to reference data",
            ReviewState::NotCompared => "Can be matched",
            ReviewState::Ambiguous { .. }
            | ReviewState::Conflict { .. }
            | ReviewState::NoSystem => "Needs your choice",
            ReviewState::NoMatch(_) => "No match found",
            ReviewState::NoData { .. } => "Identification data missing",
        }
    }

    /// The headline the review page leads with.
    pub(super) fn headline(&self) -> &'static str {
        match &self.state {
            ReviewState::Verified(_) => "Verified",
            ReviewState::ReferenceMatched => "Matched to reference data",
            ReviewState::Conflict { .. } => "The evidence disagrees",
            ReviewState::Ambiguous { .. } => "We found more than one possible match",
            ReviewState::NoSystem => "This game needs a system",
            ReviewState::NoData { .. } => {
                "Identification data for this system is not installed yet"
            }
            ReviewState::NotCompared => {
                "This game has not been compared with identification data yet"
            }
            ReviewState::NoMatch(_) => "No trusted match was found for this file",
        }
    }

    /// Plain-language reason a game did not verify automatically.
    pub(super) fn explanation(&self) -> String {
        let special = self.apparent_release.map(|label| {
            format!(
                " This appears to be a {} release. It may not exist in the standard identification data; that is not a fault with the file.",
                label.to_ascii_lowercase()
            )
        });
        let base = match &self.state {
            ReviewState::Verified(facts) if facts.by_file_evidence => {
                "EmuWiz verified this game from the file's own contents.".to_string()
            }
            ReviewState::Verified(_) => {
                "Its exact fingerprint matches one entry in trusted identification data.".to_string()
            }
            ReviewState::ReferenceMatched => {
                "This set was checked against the MAME reference data.".to_string()
            }
            ReviewState::Conflict { detail } => format!(
                "Trusted evidence disagrees about what this is ({detail}). EmuWiz will not pick one for you."
            ),
            ReviewState::Ambiguous { .. } => {
                "More than one trusted entry fits this file. EmuWiz will not guess between them."
                    .to_string()
            }
            ReviewState::NoSystem => {
                "EmuWiz does not know which system this file belongs to, so it cannot look it up. Choose the system first."
                    .to_string()
            }
            ReviewState::NoData { reference_source_exists: true } => {
                "Install identification data for this system and EmuWiz will match the game automatically."
                    .to_string()
            }
            ReviewState::NoData { reference_source_exists: false } => {
                "There is no known trusted identification database for this system, so its games cannot be verified this way."
                    .to_string()
            }
            ReviewState::NotCompared => {
                "Nothing is wrong. Run the identification check for this system and EmuWiz will compare the file with trusted data.".to_string()
            }
            ReviewState::NoMatch(NoMatchReason::NotInData) => {
                "The installed data was searched and does not contain this exact file. That does not mean the file is bad.".to_string()
            }
            ReviewState::NoMatch(NoMatchReason::WeakEvidenceOnly) => {
                "Only a weak clue matched (a short checksum or the file name). That is not enough to verify a game.".to_string()
            }
            ReviewState::NoMatch(NoMatchReason::NoUsableEvidence) => {
                "EmuWiz could not read enough of this file to compare it.".to_string()
            }
        };
        match (&self.state, special) {
            (ReviewState::Verified(_), _) => base,
            (_, Some(special)) => format!("{base}{special}"),
            (_, None) => base,
        }
    }
}

/// The single place a game's identity state is decided. Pure: the same inputs
/// give the same answer on every page. `dat` is the recorded DAT audit result
/// for this game when it has been loaded. A saved exact DAT match is also
/// carried on the game itself (`Game::dat_exact`), set when the library loads.
pub(super) fn review_for(game: &Game, ctx: &IdentityContext, dat: Option<&DatKnowledge>) -> Review {
    let platform = (game.platform != UNKNOWN_PLATFORM).then_some(game.platform.as_str());
    let facts = IdentityFacts {
        platform,
        relative_path: &game.archive.relative_path,
        report: game.archive.identity_report.as_ref(),
        matched_by_reference_data: ctx.matched.contains(&game.archive.id),
    };
    let class = classify_identity(&facts, ctx.inventory.as_ref());
    let name = game.archive.relative_path.to_string_lossy();
    let apparent_release = release_class(&name);
    let usable_dat = dat.filter(|knowledge| knowledge.trusted_source && !knowledge.stale);

    // 1. Verified: the file's own evidence, or one exact authoritative hash match.
    let dat_verified = game.dat_exact.is_some()
        || usable_dat
            .is_some_and(|k| matches!(k.state, DatVerificationState::VerifiedSingleMatch { .. }));
    if class == IdentityAttention::Identified || dat_verified {
        let canonical = usable_dat
            .filter(|k| matches!(k.state, DatVerificationState::VerifiedSingleMatch { .. }));
        let title = canonical
            .and_then(|k| k.title.clone())
            .unwrap_or_else(|| game.title.clone());
        return Review {
            state: ReviewState::Verified(VerifiedFacts {
                release: release_class(&title).or(apparent_release),
                dump: DumpQuality::from_dat_name(&title),
                region: canonical.and_then(|k| k.region.clone()),
                source: canonical
                    .map(|k| k.source_name.clone())
                    .or_else(|| game.dat_exact.flatten().map(str::to_string)),
                by_file_evidence: class == IdentityAttention::Identified
                    && game.dat_exact.is_none(),
                title,
            }),
            apparent_release: None,
        };
    }
    let state = match (&class, usable_dat.map(|k| &k.state)) {
        (IdentityAttention::NeedsChoice(ChoiceReason::Conflict), _) => ReviewState::Conflict {
            detail: "the file's own evidence conflicts".into(),
        },
        (_, Some(DatVerificationState::Conflicting { detail })) => ReviewState::Conflict {
            detail: detail.clone(),
        },
        (IdentityAttention::NeedsChoice(ChoiceReason::Ambiguous), _) => ReviewState::Ambiguous {
            candidates: usable_dat.map(|k| k.candidates.clone()).unwrap_or_default(),
        },
        (_, Some(DatVerificationState::AmbiguousMultipleCandidates { .. })) => {
            ReviewState::Ambiguous {
                candidates: usable_dat.map(|k| k.candidates.clone()).unwrap_or_default(),
            }
        }
        (IdentityAttention::NeedsChoice(ChoiceReason::SystemUnknown), _) => ReviewState::NoSystem,
        (_, Some(DatVerificationState::NoMatch)) => ReviewState::NoMatch(NoMatchReason::NotInData),
        (
            _,
            Some(DatVerificationState::Probable | DatVerificationState::FilenameOnlyNotVerified),
        ) => ReviewState::NoMatch(NoMatchReason::WeakEvidenceOnly),
        (_, Some(DatVerificationState::NoUsableEvidence)) => {
            ReviewState::NoMatch(NoMatchReason::NoUsableEvidence)
        }
        (IdentityAttention::SetupRequired(_), _) => ReviewState::NoData {
            reference_source_exists: true,
        },
        (IdentityAttention::Informational(InformationalReason::NoReferenceSource), _) => {
            ReviewState::NoData {
                reference_source_exists: false,
            }
        }
        (IdentityAttention::Informational(InformationalReason::MatchedByReferenceData), _) => {
            ReviewState::ReferenceMatched
        }
        _ => ReviewState::NotCompared,
    };
    Review {
        state,
        apparent_release,
    }
}

#[cfg(test)]
mod tests;
