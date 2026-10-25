//! Disc order taken from verified DAT identities, never from local file names.
//!
//! A disc, disk or tape's position comes from the title of the one catalogue
//! entry its own bytes matched by a cryptographic hash, for example
//! `"Final Fantasy VII (USA) (Disc 2)"`. Only exact, delimited forms are read:
//! `(Disc N)`, `(Disk N)` or `(Tape N)`, optionally `of M` as TOSEC writes
//! them. The unit must fit the image: disc numbers for optical images, disk
//! numbers for floppies, tape numbers for tapes. Titles naming a side stay
//! unverified. The highest number seen is never a total. A total comes either
//! from the title's own `of M`, or from the same installed catalogue revision
//! declaring exactly the entries 1..M of that unit under the same base title. Anything else leaves the set without a known total, so the
//! engine keeps it unverified and Launch never builds a playlist for it.
use super::model::*;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Namespace prefix of every release key this producer writes. Readers use it
/// to recheck that saved order still rests on a current DAT identity.
pub const DAT_DISC_ORDER_NAMESPACE: &str = "dat-disc";
/// Producer name recorded with persisted topology rows.
pub const DAT_DISC_ORDER_PRODUCER: &str = "dat-disc-order";
/// The media family a catalogued image belongs to, when this producer can
/// order it. Track files (`.bin`) are companions of a CUE sheet, never media
/// of their own here.
pub fn dat_order_family(format: &str) -> Option<MediaFamily> {
    (!format.eq_ignore_ascii_case("bin"))
        .then(|| super::adapters::family_for_format(format))
        .flatten()
}

/// The position a catalogue title states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatDiscToken {
    /// The title with the position token removed: the release identity.
    pub base_title: String,
    pub unit: OrdinalUnit,
    pub number: u16,
    /// Only `(Disc N of M)`-style titles state a total themselves.
    pub total: Option<u16>,
}

impl DatDiscToken {
    /// Disc numbers order optical images, disk numbers floppies and tape
    /// numbers tapes. Any other pairing is not evidence.
    pub fn fits(&self, family: MediaFamily) -> bool {
        matches!(
            (self.unit, family),
            (OrdinalUnit::Disc, MediaFamily::Optical)
                | (OrdinalUnit::Disk, MediaFamily::Floppy)
                | (OrdinalUnit::Tape, MediaFamily::Tape)
        )
    }
}

fn collapse_spaces(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn number(value: &str) -> Option<u16> {
    value
        .chars()
        .all(|c| c.is_ascii_digit())
        .then(|| value.parse::<u16>().ok())
        .flatten()
        .filter(|n| (1..=4096).contains(n))
}

/// Reads one catalogue title's position token: `(Disc N)`, `(Disk N)` or
/// `(Tape N)`, optionally `of M`. `None` unless the title has exactly one
/// such token, no other group starting with one of those words, and no side
/// token: sides cannot be ordered from a title alone.
pub fn dat_disc_token(name: &str) -> Option<DatDiscToken> {
    let mut found = None;
    let mut rest = name;
    let mut offset = 0;
    while let Some(open) = rest.find(['(', '[']) {
        let after = &rest[open + 1..];
        let Some(close) = after.find([')', ']']) else {
            break;
        };
        let inner = &after[..close];
        let words = inner.split_whitespace().collect::<Vec<_>>();
        let first = words.first().map(|w| w.to_ascii_lowercase());
        if first.as_deref() == Some("side") || words.iter().any(|w| w.eq_ignore_ascii_case("side"))
        {
            return None;
        }
        let unit = match first.as_deref() {
            Some("disc") => Some(OrdinalUnit::Disc),
            Some("disk") => Some(OrdinalUnit::Disk),
            Some("tape") => Some(OrdinalUnit::Tape),
            _ => None,
        };
        if let Some(unit) = unit {
            let (position, total) = match words.as_slice() {
                [_, n] => (number(n)?, None),
                [_, n, of, m] if of.eq_ignore_ascii_case("of") => {
                    let (n, m) = (number(n)?, number(m)?);
                    (m > 1 && n <= m).then_some(())?;
                    (n, Some(m))
                }
                _ => return None,
            };
            if found.is_some() {
                return None;
            }
            let start = offset + open;
            found = Some((unit, position, total, start, start + inner.len() + 2));
        }
        offset += open + 1 + close + 1;
        rest = &after[close + 1..];
    }
    let (unit, number, total, start, end) = found?;
    let base_title = collapse_spaces(&format!("{}{}", &name[..start], &name[end..]));
    (!base_title.is_empty()).then_some(DatDiscToken {
        base_title,
        unit,
        number,
        total,
    })
}

/// The total a catalogue revision declares for `base_title`: its `(Disc N)`
/// entries of the same unit must be exactly 1..=M with M of at least two.
/// Gaps, repeats, or a sibling that states its own total make it unknown.
pub fn catalogue_disc_total<'a>(
    token: &DatDiscToken,
    catalogue_names: impl IntoIterator<Item = &'a str>,
) -> Option<u16> {
    let mut numbers = BTreeSet::new();
    for name in catalogue_names {
        let Some(sibling) = dat_disc_token(name) else {
            continue;
        };
        if sibling.base_title != token.base_title || sibling.unit != token.unit {
            continue;
        }
        if sibling.total.is_some() || !numbers.insert(sibling.number) {
            return None;
        }
    }
    let total = numbers.len() as u16;
    (total >= 2 && numbers.iter().copied().eq(1..=total)).then_some(total)
}

/// One loose media image whose own bytes matched one current catalogue entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatDiscCandidate {
    pub path: PathBuf,
    /// Canonical platform id from the catalogue.
    pub platform: String,
    pub format: String,
    pub source_id: String,
    pub source_revision: String,
    pub catalogue_sha256: Option<String>,
    pub canonical_name: String,
    pub matched_algorithm: String,
    pub matched_value: Option<String>,
}

/// Release key namespace for one catalogue source and revision.
pub fn dat_disc_namespace(source_id: &str, source_revision: &str) -> String {
    format!("{DAT_DISC_ORDER_NAMESPACE}:{source_id}:{source_revision}")
}

/// A topology record carrying only catalogue evidence. Local file names add
/// nothing, so a misleading name can neither help nor split a set.
pub fn dat_disc_record(
    candidate: &DatDiscCandidate,
    family: MediaFamily,
    token: &DatDiscToken,
    total: Option<u16>,
) -> MediaRecord {
    let mut namespace = dat_disc_namespace(&candidate.source_id, &candidate.source_revision);
    if let Some(digest) = &candidate.catalogue_sha256 {
        namespace.push_str(&format!(":sha256:{digest}"));
    }
    let mut evidence = MediaEvidence::new(EvidenceKind::TrustedDat, &candidate.source_id);
    evidence.provenance.version = Some(candidate.source_revision.clone());
    evidence.release = Some(IdentityKey::new(namespace.clone(), &token.base_title));
    evidence.medium = Some(IdentityKey::new(namespace, &candidate.canonical_name));
    evidence.title = Some(token.base_title.clone());
    evidence.ordinal = Some(MediaOrdinal {
        number: token.number,
        unit: token.unit,
    });
    // A numbered Disc/Disk/Tape entry is the game's own media; bonus or
    // extras discs carry other tokens and are never read here.
    evidence.role = Some(MediaRole::GameMedia);
    evidence.expected_count = total.map(|count| ExpectedCount {
        count,
        unit: token.unit,
    });
    evidence.notes.push(format!(
        "{:?} {} is stated by catalogue entry {:?}, matched exactly by {}{}",
        token.unit,
        token.number,
        candidate.canonical_name,
        candidate.matched_algorithm,
        candidate
            .matched_value
            .as_deref()
            .map(|value| format!(" {value}"))
            .unwrap_or_default()
    ));
    if let Some(sha256) = &candidate.catalogue_sha256 {
        evidence
            .notes
            .push(format!("Catalogue file SHA-256 {sha256}"));
    }
    evidence.notes.push(match (token.total, total) {
        (Some(_), _) => "The total is stated by the catalogue entry itself".into(),
        (None, Some(_)) => {
            "The total is the catalogue revision's own complete list of media for this title".into()
        }
        (None, None) => "The catalogue does not establish how many discs this title has".into(),
    });
    MediaRecord {
        source: MediaSource {
            path: candidate.path.clone(),
            archive_member: None,
        },
        platform: Some(candidate.platform.clone()),
        family: Some(family),
        format: candidate.format.clone(),
        availability: MediaAvailability::Observed,
        evidence: vec![evidence],
        warnings: Vec::new(),
    }
}

/// Whether a DAT result rests on the file's content (a hash) and so may
/// contest a claimed entry. Filename-only, no-match and no-evidence results
/// never do: local names can neither help nor split a set.
pub fn content_evidence(
    state: &crate::dat::library_identity_summary::DatVerificationState,
) -> bool {
    use crate::dat::library_identity_summary::DatVerificationState as S;
    matches!(
        state,
        S::VerifiedSingleMatch { .. }
            | S::Probable
            | S::AmbiguousMultipleCandidates { .. }
            | S::Conflicting { .. }
    )
}

/// Whether `set` is this producer's output.
pub fn is_dat_disc_order_set(set: &MediaSet) -> bool {
    set.identity
        .key
        .namespace
        .starts_with(&format!("{DAT_DISC_ORDER_NAMESPACE}:"))
}

/// The catalogue claim one representation rests on: (source, revision, entry).
pub fn dat_disc_claim(representation: &MediaRepresentation) -> Option<(String, String, String)> {
    let evidence = representation.record.evidence.iter().find(|e| {
        e.provenance.kind == EvidenceKind::TrustedDat
            && e.medium.as_ref().is_some_and(|m| {
                m.namespace
                    .starts_with(&format!("{DAT_DISC_ORDER_NAMESPACE}:"))
            })
    })?;
    Some((
        evidence.provenance.source.clone(),
        evidence.provenance.version.clone()?,
        evidence.medium.as_ref()?.value.clone(),
    ))
}

/// Exact catalogue digest encoded in this producer's typed identity namespace.
/// Historical order records carry none and cannot back a digest-bound identity.
pub fn dat_disc_catalogue_sha256(representation: &MediaRepresentation) -> Option<&str> {
    let medium = representation
        .record
        .evidence
        .iter()
        .find(|e| {
            e.provenance.kind == EvidenceKind::TrustedDat
                && e.medium.as_ref().is_some_and(|m| {
                    m.namespace
                        .starts_with(&format!("{DAT_DISC_ORDER_NAMESPACE}:"))
                })
        })?
        .medium
        .as_ref()?;
    let (_, digest) = medium.namespace.rsplit_once(":sha256:")?;
    (digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit())).then_some(digest)
}

/// Why a catalogue-ordered set is not complete enough for a playlist, in
/// words a player can act on. `None` for other producers' sets, and for
/// complete ones.
pub fn dat_order_gap(set: &MediaSet) -> Option<String> {
    if !is_dat_disc_order_set(set) {
        return None;
    }
    let noun = match set.family {
        Some(MediaFamily::Floppy) => "disk",
        Some(MediaFamily::Tape) => "tape",
        _ => "disc",
    };
    let blocking = |kind| set.conflicts.iter().any(|c| c.blocking && c.kind == kind);
    if blocking(ConflictKind::UnknownCount) {
        Some(format!(
            "the catalogue confirms each {noun}'s number but not how many {noun}s the game has, so the set cannot be checked as complete"
        ))
    } else if blocking(ConflictKind::MissingMedium) {
        Some(format!(
            "the catalogue lists a {noun} of this game that is not in the library"
        ))
    } else {
        None
    }
}

#[cfg(test)]
#[path = "dat_disc_order_tests.rs"]
mod tests;
