//! Read-only audit using hashes already known to EmuWiz.
//!
//! Every verdict is derived from a comparison between what the DAT file claims
//! and what EmuWiz already knows. Nothing here hashes a local file: the
//! caller supplies known hashes and size, and the audit logic compares them
//! against the indexed DAT entries.
//!
//! # Verdict rules
//!
//! - **Exact**: SHA-256, SHA-1, or MD5 matches exactly one DAT entry.
//! - **ExactMultipleCandidates**: a cryptographic hash matches several entries.
//! - **Probable**: CRC32 (with size, where both are known) matches one entry.
//! - **ProbableMultipleCandidates**: CRC32 matches several entries.
//! - **FilenameOnly**: filename matches, and no hash evidence was available.
//! - **Ambiguous**: candidates exist, but the evidence conflicts.
//! - **NotInDat**: every hash that could be compared found no candidate.
//! - **NoUsableEvidence**: no reliable content evidence to compare (including
//!   archive contents that could not be examined).
//!
//! # Every shared algorithm is consulted, and their candidates are unioned
//!
//! A DAT carries the algorithms its publisher chose; the caller knows whatever
//! EmuWiz happens to have computed. The two sets overlap but neither contains
//! the other - a No-Intro DAT publishes CRC32/MD5/SHA-1 and no SHA-256, so
//! stopping at the strongest hash the *caller* holds reports a perfectly matching
//! file as absent from the catalogue. Worse, *returning* on the first algorithm
//! that finds something hides every record that publishes only a weaker one
//! (S03-01): two entries with identical content became a "unique" match for
//! whichever carried the stronger hash. [`collect_crypto_candidates`] therefore
//! looks every algorithm up and reports the deduplicated union, in catalogue
//! order, labelled with the strongest algorithm that confirmed a record. Only
//! candidates whose shared, well-formed cryptographic hashes agree may decide a
//! match; a CRC32 match cannot override such a contradiction. Missing algorithms
//! are not contradictions. This compares supplied evidence, not newly read file
//! content, and retains the existing confidence tiers.
//!
//! # `nodump` and `baddump` records are never identity
//!
//! A catalogue record whose `status` is `nodump` or `baddump` describes
//! content the publisher could not or would not verify (S03-03). A file whose
//! hash agrees with such a record is a known bad dump or a placeholder hit,
//! not the game, so the verdict is `Ambiguous` with the record named - never
//! `Exact` or `Probable`. Weaker evidence is not consulted afterwards.

use serde::Serialize;

use super::index::DatIndex;

/// The outcome of comparing a single known file against a DAT index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditVerdict {
    /// SHA-256, SHA-1, or MD5 exact match against exactly one DAT entry.
    Exact {
        game_name: String,
        rom_name: String,
        algorithm: &'static str,
    },
    /// A cryptographic hash matches multiple DAT entries.
    ExactMultipleCandidates {
        algorithm: &'static str,
        count: usize,
        game_names: Vec<String>,
    },
    /// CRC32 plus exact size match.
    Probable { game_name: String, rom_name: String },
    /// CRC32 (with or without size) matches several DAT entries.
    ///
    /// Deliberately *not* `ExactMultipleCandidates`: CRC32 is a 32-bit checksum,
    /// and several entries sharing one is as likely to be a collision as a real
    /// set of identical dumps. Reporting it as an "Exact" verdict would claim a
    /// confidence the evidence does not support.
    ProbableMultipleCandidates {
        algorithm: &'static str,
        count: usize,
        game_names: Vec<String>,
    },
    /// Filename matches, but no hash to confirm.
    FilenameOnly { game_name: String, rom_name: String },
    /// Candidate exists, but evidence conflicts.
    Ambiguous { detail: String },
    /// Known hash has no candidate.
    NotInDat,
    /// No reliable content evidence to compare; outer archive hashes may exist.
    NoUsableEvidence,
}

impl AuditVerdict {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Exact { .. } => "Exact",
            Self::ExactMultipleCandidates { .. } => "Exact (multiple)",
            Self::ProbableMultipleCandidates { .. } => "Probable (multiple)",
            Self::Probable { .. } => "Probable",
            Self::FilenameOnly { .. } => "Filename only",
            Self::Ambiguous { .. } => "Ambiguous",
            Self::NotInDat => "Not in DAT",
            Self::NoUsableEvidence => "No usable evidence",
        }
    }

    /// Whether the verdict rests on a cryptographic hash.
    ///
    /// CRC32 verdicts are excluded however many entries they matched: a 32-bit
    /// checksum agreeing is not the same kind of evidence as SHA-1 agreeing.
    pub fn is_confident(&self) -> bool {
        matches!(
            self,
            Self::Exact { .. } | Self::ExactMultipleCandidates { .. }
        )
    }
}

/// One audited item: a local file compared against the DAT.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditEntry {
    pub local_path: String,
    pub local_filename: String,
    pub verdict: AuditVerdict,
}

/// The result of an audit pass over a set of local files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditReport {
    pub entries: Vec<AuditEntry>,
    pub summary: AuditSummary,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AuditSummary {
    pub total: usize,
    pub exact: usize,
    pub exact_multiple: usize,
    pub probable: usize,
    pub filename_only: usize,
    pub probable_multiple: usize,
    pub ambiguous: usize,
    pub not_in_dat: usize,
    pub no_evidence: usize,
}

/// Known hashes and metadata for a single local file.
///
/// The caller populates this from existing EmuWiz data — no local
/// hashing is performed inside this module.
#[derive(Debug, Clone, Default)]
pub struct KnownFileEvidence {
    pub filepath: String,
    pub filename: String,
    pub size_bytes: Option<u64>,
    pub crc32: Option<String>,
    pub md5: Option<String>,
    pub sha1: Option<String>,
    pub sha256: Option<String>,
}

impl KnownFileEvidence {
    pub fn new(filepath: impl Into<String>, filename: impl Into<String>) -> Self {
        Self {
            filepath: filepath.into(),
            filename: filename.into(),
            ..Default::default()
        }
    }

    pub fn with_size(mut self, size: u64) -> Self {
        self.size_bytes = Some(size);
        self
    }

    pub fn with_crc32(mut self, crc: impl Into<String>) -> Self {
        self.crc32 = Some(crc.into());
        self
    }

    pub fn with_md5(mut self, md5: impl Into<String>) -> Self {
        self.md5 = Some(md5.into());
        self
    }

    pub fn with_sha1(mut self, sha1: impl Into<String>) -> Self {
        self.sha1 = Some(sha1.into());
        self
    }

    pub fn with_sha256(mut self, sha256: impl Into<String>) -> Self {
        self.sha256 = Some(sha256.into());
        self
    }
}

/// Audits a set of known file evidence against a DAT index.
///
/// For each file, tries the strongest hash first (SHA-256 -> SHA-1 -> MD5),
/// then falls back to CRC32+size, then filename.
pub fn audit_files(known: &[KnownFileEvidence], index: &DatIndex) -> AuditReport {
    let mut entries = Vec::with_capacity(known.len());

    for file in known {
        let verdict = audit_one(file, index);
        let filename = std::path::Path::new(&file.filepath)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| file.filename.clone());
        entries.push(AuditEntry {
            local_path: file.filepath.clone(),
            local_filename: filename,
            verdict,
        });
    }

    let summary = build_summary(&entries);

    AuditReport { entries, summary }
}

/// What a catalogue record's `status` attribute says about its hashes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DumpStatus {
    /// `good`, `verified`, or no status: the hashes describe a real dump.
    Ordinary,
    /// `nodump`: the catalogue has never seen a dump; any published hash is
    /// a placeholder and can never confirm a file.
    NoDump,
    /// `baddump`: the catalogue knows this exact content and knows it is
    /// wrong. A file matching it is identified as a known bad dump, never
    /// as the game.
    BadDump,
}

impl DumpStatus {
    pub fn of(status: Option<&str>) -> Self {
        match status.map(str::trim) {
            Some(value) if value.eq_ignore_ascii_case("nodump") => Self::NoDump,
            Some(value) if value.eq_ignore_ascii_case("baddump") => Self::BadDump,
            _ => Self::Ordinary,
        }
    }

    /// Whether a record with this status may stand as identity evidence.
    pub fn is_verifiable(self) -> bool {
        matches!(self, Self::Ordinary)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ordinary => "good",
            Self::NoDump => "nodump",
            Self::BadDump => "baddump",
        }
    }
}

/// Every catalogue record that one of the caller's cryptographic hashes
/// agrees with, collected across **all** shared algorithms.
///
/// S03-01 / D3-01: stopping at the first algorithm that found candidates
/// hid any record that publishes only a weaker algorithm. Two catalogue
/// entries describing byte-identical content - one with SHA-1 and MD5, one
/// with MD5 alone - used to collapse into a unique `Exact` match for the
/// first, because the SHA-1 lookup happened to return before the MD5 lookup
/// ran. Identity certainty was decided by which hashes each publisher chose
/// to print. Here every algorithm is consulted and the union is reported,
/// each record once, in catalogue order, so the ambiguity stays visible.
#[derive(Debug, Clone, Default)]
pub struct CryptoCandidates {
    /// Verifiable records whose shared cryptographic hashes all agree with
    /// the caller's. Deduplicated, in catalogue declaration order.
    pub consistent: Vec<super::index::SharedRomRef>,
    /// The strongest algorithm that confirmed at least one consistent
    /// record. `None` when `consistent` is empty.
    pub algorithm: Option<&'static str>,
    /// Records a hash matched whose status marks them `nodump`/`baddump`.
    /// They are identity evidence of the *wrong* kind and never become a
    /// match, however strong the hash. Deduplicated, in catalogue order.
    pub unverifiable: Vec<(super::index::SharedRomRef, DumpStatus)>,
    /// Matched records excluded because another cryptographic hash they
    /// publish disagrees with the caller's value for the same algorithm.
    pub contradicted: usize,
    /// At least one well-formed cryptographic hash was looked up.
    pub compared_any: bool,
}

impl CryptoCandidates {
    /// A one-line account of the excluded `nodump`/`baddump` records.
    pub fn unverifiable_detail(&self) -> String {
        self.unverifiable
            .iter()
            .map(|(record, status)| {
                format!(
                    "{} / {} ({})",
                    record.game_name,
                    record.rom_name,
                    status.label()
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    }
}

const CRYPTO_ALGORITHMS: [&str; 3] = ["SHA-256", "SHA-1", "MD5"];

fn known_value<'a>(known: &'a KnownFileEvidence, algorithm: &str) -> Option<&'a str> {
    match algorithm {
        "SHA-256" => known.sha256.as_deref(),
        "SHA-1" => known.sha1.as_deref(),
        _ => known.md5.as_deref(),
    }
}

fn member_order_key(record: &super::index::DatRomRef) -> (usize, u8, usize, usize, usize) {
    use super::index::MemberLocation;
    match record.member_key.location {
        MemberLocation::TopLevel { rom_index } => (record.game_index, 0, rom_index, 0, 0),
        MemberLocation::DataArea {
            part_index,
            data_area_index,
            member_index,
        } => (
            record.game_index,
            1,
            part_index,
            data_area_index,
            member_index,
        ),
    }
}

/// Collects [`CryptoCandidates`] for `known` against `index`.
///
/// This is the single place the audit, the policy annotation and the
/// persisted matched-reference list derive their candidate set from, so
/// none of them can disagree about what the file matched.
pub fn collect_crypto_candidates(known: &KnownFileEvidence, index: &DatIndex) -> CryptoCandidates {
    let mut out = CryptoCandidates::default();
    for algorithm in CRYPTO_ALGORITHMS {
        let Some(value) = known_value(known, algorithm) else {
            continue;
        };
        let Some(normalised) = normalise_for_lookup(value, algorithm) else {
            continue;
        };
        out.compared_any = true;
        let candidates = match algorithm {
            "SHA-256" => index.lookup_sha256(&normalised),
            "SHA-1" => index.lookup_sha1(&normalised),
            _ => index.lookup_md5(&normalised),
        };
        for candidate in candidates {
            if cryptographic_hash_disagrees(known, candidate) {
                out.contradicted += 1;
                continue;
            }
            let status = DumpStatus::of(candidate.status.as_deref());
            if !status.is_verifiable() {
                if !out
                    .unverifiable
                    .iter()
                    .any(|(seen, _)| seen.key() == candidate.key())
                {
                    out.unverifiable.push((candidate.clone(), status));
                }
                continue;
            }
            if out
                .consistent
                .iter()
                .any(|seen| seen.key() == candidate.key())
            {
                continue;
            }
            if out.algorithm.is_none() {
                // Algorithms are visited strongest first, so the first one to
                // confirm a consistent record is the strongest that did.
                out.algorithm = Some(algorithm);
            }
            out.consistent.push(candidate.clone());
        }
    }
    out.consistent
        .sort_by_key(|record| member_order_key(record));
    out.unverifiable
        .sort_by_key(|(record, _)| member_order_key(record));
    out
}

pub(crate) fn audit_one(known: &KnownFileEvidence, index: &DatIndex) -> AuditVerdict {
    let crypto = collect_crypto_candidates(known, index);
    if !crypto.consistent.is_empty() {
        return handle_candidates(&crypto.consistent, crypto.algorithm.unwrap_or("SHA-1"));
    }
    if !crypto.unverifiable.is_empty() {
        // S03-03 / D3-03: the hash agrees with a record the catalogue itself
        // declares unusable. That is a strong statement about what this file
        // is - a known bad dump, or a placeholder - and it must never be
        // reported as the game. Nothing weaker (CRC32, filename) is consulted
        // afterwards: it could only name the same record or a collision.
        return AuditVerdict::Ambiguous {
            detail: format!(
                "the only catalogue record(s) matching this file's cryptographic hash are \
                 marked unverifiable: {}; a bad dump or a missing dump is never a verified \
                 identity",
                crypto.unverifiable_detail()
            ),
        };
    }
    let crypto_conflict = crypto.contradicted > 0;
    let mut compared_any = crypto.compared_any;

    // CRC32, qualified by size when both are known.
    if let Some(crc) = known.crc32.as_deref()
        && let Some(normalised) = normalise_for_lookup(crc, "CRC32")
    {
        compared_any = true;
        let candidates = index.lookup_crc32(&normalised);
        if !candidates.is_empty() {
            // A 32-bit checksum agreeing proves nothing once a stronger hash
            // the DAT publishes for that same entry says otherwise. Such an
            // entry is not a candidate at all, with or without a size match.
            let uncontradicted: Vec<_> = candidates
                .iter()
                .filter(|candidate| !cryptographic_hash_disagrees(known, candidate))
                .collect();
            if uncontradicted.is_empty() {
                return AuditVerdict::Ambiguous {
                    detail: format!(
                        "CRC32 {normalised} matches {} DAT entry(s), but the stronger hashes \
                         published for them disagree with this file",
                        candidates.len()
                    ),
                };
            }
            // A CRC32 agreeing with a `nodump`/`baddump` record is the same
            // wrong-kind evidence as above, only weaker.
            let (verifiable, unverifiable): (
                Vec<&super::index::SharedRomRef>,
                Vec<&super::index::SharedRomRef>,
            ) = uncontradicted
                .iter()
                .partition(|candidate| DumpStatus::of(candidate.status.as_deref()).is_verifiable());
            if verifiable.is_empty() {
                let detail = unverifiable
                    .iter()
                    .map(|candidate: &&super::index::SharedRomRef| {
                        format!(
                            "{} / {} ({})",
                            candidate.game_name,
                            candidate.rom_name,
                            DumpStatus::of(candidate.status.as_deref()).label()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                return AuditVerdict::Ambiguous {
                    detail: format!(
                        "CRC32 {normalised} matches only catalogue record(s) marked unverifiable: \
                         {detail}; a bad dump or a missing dump is never a verified identity"
                    ),
                };
            }
            let uncontradicted = verifiable;
            return match known.size_bytes {
                Some(size) => {
                    let matched: Vec<_> = uncontradicted
                        .iter()
                        .filter(|r| r.size_bytes == Some(size))
                        .collect();
                    match matched.len() {
                        1 => AuditVerdict::Probable {
                            game_name: matched[0].game_name.clone(),
                            rom_name: matched[0].rom_name.clone(),
                        },
                        0 => AuditVerdict::Ambiguous {
                            detail: format!(
                                "CRC32 {normalised} matches {} DAT entry(s), but size {size} \
                                 disagrees with all of them",
                                uncontradicted.len()
                            ),
                        },
                        _ => AuditVerdict::ProbableMultipleCandidates {
                            algorithm: "CRC32+size",
                            count: matched.len(),
                            game_names: matched.iter().map(|r| r.game_name.clone()).collect(),
                        },
                    }
                }
                None => match uncontradicted.len() {
                    1 => AuditVerdict::Probable {
                        game_name: uncontradicted[0].game_name.clone(),
                        rom_name: uncontradicted[0].rom_name.clone(),
                    },
                    _ => AuditVerdict::ProbableMultipleCandidates {
                        algorithm: "CRC32",
                        count: uncontradicted.len(),
                        game_names: uncontradicted.iter().map(|r| r.game_name.clone()).collect(),
                    },
                },
            };
        }
    }

    if crypto_conflict {
        return AuditVerdict::Ambiguous {
            detail: "Cryptographic hash matches exist, but their other published cryptographic hashes disagree with this file".into(),
        };
    }

    // Something was comparable and none of it found anything: the file is not in
    // this DAT. Falling through to the filename here would dress a name collision
    // up as a match after the hashes had already said otherwise.
    if compared_any {
        return AuditVerdict::NotInDat;
    }

    // No hash at all. A filename match is worth reporting, but only as itself.
    let filename = std::path::Path::new(&known.filepath)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| known.filename.clone());
    if !filename.is_empty() {
        let candidates = index.lookup_filename(&filename);
        if !candidates.is_empty() {
            return AuditVerdict::FilenameOnly {
                game_name: candidates[0].game_name.clone(),
                rom_name: candidates[0].rom_name.clone(),
            };
        }
    }

    AuditVerdict::NoUsableEvidence
}

/// Whether `candidate` publishes a cryptographic hash that differs from
/// the value the caller holds for that same algorithm.
///
/// Only a like-for-like comparison counts: an algorithm either side lacks,
/// or a caller value that is not well-formed, contradicts nothing.
pub(crate) fn cryptographic_hash_disagrees(
    known: &KnownFileEvidence,
    candidate: &super::index::DatRomRef,
) -> bool {
    use super::model::ChecksumAlgorithm;
    [
        (
            known.sha256.as_deref(),
            "SHA-256",
            ChecksumAlgorithm::Sha256,
        ),
        (known.sha1.as_deref(), "SHA-1", ChecksumAlgorithm::Sha1),
        (known.md5.as_deref(), "MD5", ChecksumAlgorithm::Md5),
    ]
    .into_iter()
    .any(|(value, label, algorithm)| {
        let Some(held) = value.and_then(|value| normalise_for_lookup(value, label)) else {
            return false;
        };
        candidate
            .checksums
            .iter()
            .any(|published| published.algorithm == algorithm && published.value != held)
    })
}

/// Normalises a caller-supplied hash the same way the index normalised the DAT's.
///
/// The index is keyed on lowercase hex of a fixed length, because that is what
/// the parsers store. A caller holding the same hash in uppercase - which is how
/// plenty of tools print them - would otherwise miss every entry and be told the
/// file is not in the DAT. A value that is not a well-formed hash for its
/// algorithm is not looked up at all.
pub(crate) fn normalise_for_lookup(value: &str, algorithm: &str) -> Option<String> {
    use super::hash::{normalise_crc32, normalise_md5, normalise_sha1, normalise_sha256};
    match algorithm {
        "SHA-256" => normalise_sha256(value),
        "SHA-1" => normalise_sha1(value),
        "MD5" => normalise_md5(value),
        _ => normalise_crc32(value),
    }
}

fn handle_candidates(
    candidates: &[super::index::SharedRomRef],
    algorithm: &'static str,
) -> AuditVerdict {
    match candidates.len() {
        0 => AuditVerdict::NotInDat,
        1 => AuditVerdict::Exact {
            game_name: candidates[0].game_name.clone(),
            rom_name: candidates[0].rom_name.clone(),
            algorithm,
        },
        _ => AuditVerdict::ExactMultipleCandidates {
            algorithm,
            count: candidates.len(),
            game_names: candidates.iter().map(|r| r.game_name.clone()).collect(),
        },
    }
}

fn build_summary(entries: &[AuditEntry]) -> AuditSummary {
    let mut summary = AuditSummary {
        total: entries.len(),
        ..Default::default()
    };
    for entry in entries {
        match &entry.verdict {
            AuditVerdict::Exact { .. } => summary.exact += 1,
            AuditVerdict::ExactMultipleCandidates { .. } => summary.exact_multiple += 1,
            AuditVerdict::Probable { .. } => summary.probable += 1,
            AuditVerdict::ProbableMultipleCandidates { .. } => summary.probable_multiple += 1,
            AuditVerdict::FilenameOnly { .. } => summary.filename_only += 1,
            AuditVerdict::Ambiguous { .. } => summary.ambiguous += 1,
            AuditVerdict::NotInDat => summary.not_in_dat += 1,
            AuditVerdict::NoUsableEvidence => summary.no_evidence += 1,
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat::model::{
        DatEcosystem, DatFormat, DatGameEntry, DatPackingPolicy, DatRomEntry, DatSource, ParsedDat,
    };

    fn make_index() -> DatIndex {
        let dat = ParsedDat {
            source: DatSource {
                format: DatFormat::Logiqx,
                ecosystem: DatEcosystem::GenericLogiqx,
                file_path: "test.dat".into(),
                name: Some("Test".into()),
                description: None,
                version: None,
                author: None,
                homepage: None,
                clrmamepro_header: None,
                entry_count: 1,
                rom_count: 1,
                parse_warnings: Vec::new(),
                packing_policy: DatPackingPolicy::Standard,
            },
            games: vec![DatGameEntry {
                name: "Super Game".into(),
                description: None,
                roms: vec![DatRomEntry {
                    name: "super.bin".into(),
                    size_bytes: Some(4096),
                    crc32: Some("abcdef01".into()),
                    md5: Some("d41d8cd98f00b204e9800998ecf8427e".into()),
                    sha1: Some("da39a3ee5e6b4b0d3255bfef95601890afd80709".into()),
                    sha256: None,
                    status: None,
                    merge: None,
                    date: None,
                    loadflag: None,
                    ..Default::default()
                }],
                clone_of: None,
                sample_of: None,
                board: None,
                rebuild_to: None,
                year: None,
                manufacturer: None,
                source_file: None,
                comment: None,
                original_metadata: Default::default(),
                content_classification: Default::default(),
                unsupported_structure: false,
                ..Default::default()
            }],
        };
        DatIndex::build(&dat)
    }

    #[test]
    fn exact_match_by_md5() {
        let index = make_index();
        let known = KnownFileEvidence::new("a/b/super.bin", "super.bin")
            .with_md5("d41d8cd98f00b204e9800998ecf8427e");
        let report = audit_files(&[known], &index);
        assert_eq!(report.entries.len(), 1);
        assert!(matches!(
            report.entries[0].verdict,
            AuditVerdict::Exact { .. }
        ));
        assert_eq!(report.summary.exact, 1);
    }

    #[test]
    fn probable_by_crc32_and_size() {
        let index = make_index();
        let known = KnownFileEvidence::new("a/b/super.bin", "super.bin")
            .with_crc32("abcdef01")
            .with_size(4096);
        let report = audit_files(&[known], &index);
        assert_eq!(report.entries.len(), 1);
        assert!(matches!(
            report.entries[0].verdict,
            AuditVerdict::Probable { .. }
        ));
    }

    #[test]
    fn not_in_dat() {
        let index = make_index();
        let known = KnownFileEvidence::new("a/b/unknown.bin", "unknown.bin")
            .with_md5("00000000000000000000000000000000");
        let report = audit_files(&[known], &index);
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].verdict, AuditVerdict::NotInDat);
    }

    #[test]
    fn filename_only() {
        let index = make_index();
        let known = KnownFileEvidence::new("a/b/super.bin", "super.bin");
        let report = audit_files(&[known], &index);
        assert_eq!(report.entries.len(), 1);
        assert!(matches!(
            report.entries[0].verdict,
            AuditVerdict::FilenameOnly { .. }
        ));
    }

    #[test]
    fn no_usable_evidence() {
        let index = make_index();
        let known = KnownFileEvidence::new("a/b/nonexistent.bin", "nonexistent.bin");
        let report = audit_files(&[known], &index);
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].verdict, AuditVerdict::NoUsableEvidence);
    }
    const SHA1_A: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
    const SHA1_B: &str = "1111111111111111111111111111111111111111";
    const MD5_A: &str = "d41d8cd98f00b204e9800998ecf8427e";
    const MD5_B: &str = "22222222222222222222222222222222";
    const SHA256_A: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const SHA256_B: &str = "3333333333333333333333333333333333333333333333333333333333333333";

    fn rom(name: &str, size: u64, crc32: &str) -> DatRomEntry {
        DatRomEntry {
            name: name.into(),
            size_bytes: Some(size),
            crc32: Some(crc32.into()),
            ..Default::default()
        }
    }

    fn index_of(roms: Vec<(&str, DatRomEntry)>) -> DatIndex {
        let dat = ParsedDat {
            source: DatSource {
                format: DatFormat::Logiqx,
                ecosystem: DatEcosystem::GenericLogiqx,
                file_path: "regression.dat".into(),
                name: Some("Regression".into()),
                description: None,
                version: None,
                author: None,
                homepage: None,
                clrmamepro_header: None,
                entry_count: roms.len(),
                rom_count: roms.len(),
                parse_warnings: Vec::new(),
                packing_policy: DatPackingPolicy::Standard,
            },
            games: roms
                .into_iter()
                .map(|(game, rom)| DatGameEntry {
                    name: game.into(),
                    roms: vec![rom],
                    ..Default::default()
                })
                .collect(),
        };
        DatIndex::build(&dat)
    }

    fn verdict(known: KnownFileEvidence, index: &DatIndex) -> AuditVerdict {
        audit_files(&[known], index).entries.remove(0).verdict
    }

    fn file() -> KnownFileEvidence {
        KnownFileEvidence::new("lib/local.bin", "local.bin")
    }

    /// Same CRC32, different size: the size disagreement is reported as a
    /// conflict. It is never dropped to fall back on the CRC by itself.
    #[test]
    fn regression_same_crc_different_size_is_a_conflict_not_a_match() {
        let index = index_of(vec![("Game A", rom("a.bin", 4096, "abcdef01"))]);
        let result = verdict(file().with_crc32("abcdef01").with_size(8192), &index);
        match &result {
            AuditVerdict::Ambiguous { detail } => {
                assert!(detail.contains("size 8192 disagrees"), "{detail}");
            }
            other => panic!("expected a size conflict, got {other:?}"),
        }
        assert!(!result.is_confident());
    }

    /// Two entries share a CRC32 and differ in size: the size selects one.
    /// With no size at all the CRC cannot choose between them.
    #[test]
    fn regression_a_shared_crc_is_resolved_by_size_or_not_at_all() {
        let index = index_of(vec![
            ("Game A", rom("a.bin", 4096, "abcdef01")),
            ("Game B", rom("b.bin", 8192, "abcdef01")),
        ]);
        match verdict(file().with_crc32("abcdef01").with_size(8192), &index) {
            AuditVerdict::Probable { game_name, .. } => assert_eq!(game_name, "Game B"),
            other => panic!("expected Game B by size, got {other:?}"),
        }
        let unsized_result = verdict(file().with_crc32("abcdef01"), &index);
        assert!(
            matches!(
                unsized_result,
                AuditVerdict::ProbableMultipleCandidates { count: 2, .. }
            ),
            "{unsized_result:?}"
        );
        assert!(!unsized_result.is_confident());
    }

    /// CRC32 and size agree, but the DAT publishes a stronger hash for that
    /// entry and the file's own value for the same algorithm differs. The
    /// stronger hash has already said "this is not that ROM"; a CRC+size
    /// match must not be reported on top of it.
    #[test]
    fn regression_crc_and_size_never_override_a_disagreeing_stronger_hash() {
        let mut entry = rom("a.bin", 4096, "abcdef01");
        entry.md5 = Some(MD5_A.into());
        entry.sha1 = Some(SHA1_A.into());
        entry.sha256 = Some(SHA256_A.into());
        let index = index_of(vec![("Game A", entry)]);
        let crc = || file().with_crc32("abcdef01").with_size(4096);

        for (label, known) in [
            ("sha1", crc().with_sha1(SHA1_B)),
            ("md5", crc().with_md5(MD5_B)),
            ("sha256", crc().with_sha256(SHA256_B)),
            ("sha1+md5", crc().with_sha1(SHA1_B).with_md5(MD5_B)),
            // A missing size does not make the contradicted CRC usable.
            (
                "crc-without-size",
                file().with_crc32("abcdef01").with_sha1(SHA1_B),
            ),
            // One stronger hash agreeing in form does not excuse another:
            // here MD5 is absent from the file and SHA-256 disagrees.
            (
                "sha256-without-size",
                file().with_crc32("abcdef01").with_sha256(SHA256_B),
            ),
        ] {
            let result = verdict(known, &index);
            assert!(
                !matches!(
                    result,
                    AuditVerdict::Probable { .. }
                        | AuditVerdict::ProbableMultipleCandidates { .. }
                        | AuditVerdict::Exact { .. }
                        | AuditVerdict::ExactMultipleCandidates { .. }
                ),
                "{label}: a contradicted CRC match was reported as {result:?}"
            );
            assert!(!result.is_confident(), "{label}");
            match &result {
                AuditVerdict::Ambiguous { detail } => {
                    assert!(detail.contains("disagree"), "{label}: {detail}");
                }
                other => panic!("{label}: expected a reported conflict, got {other:?}"),
            }
        }
    }

    /// The contradicted entry is excluded; an uncontradicted entry sharing
    /// the CRC32 and size is still a (probable) candidate.
    #[test]
    fn regression_a_disagreeing_stronger_hash_only_excludes_its_own_entry() {
        let mut contradicted = rom("a.bin", 4096, "abcdef01");
        contradicted.sha1 = Some(SHA1_A.into());
        let crc_only = rom("b.bin", 4096, "abcdef01");
        let index = index_of(vec![("Game A", contradicted), ("Game B", crc_only)]);

        let result = verdict(
            file()
                .with_crc32("abcdef01")
                .with_size(4096)
                .with_sha1(SHA1_B),
            &index,
        );
        match &result {
            AuditVerdict::Probable { game_name, .. } => assert_eq!(game_name, "Game B"),
            other => panic!("expected only the uncontradicted entry, got {other:?}"),
        }
        assert!(!result.is_confident());
    }

    /// A stronger hash the file holds cannot contradict an entry that never
    /// published that algorithm: CRC32+size stays a probable match, and it is
    /// still never a confident one.
    #[test]
    fn regression_crc_and_size_remain_only_probable_when_nothing_stronger_is_published() {
        let index = index_of(vec![("Game A", rom("a.bin", 4096, "abcdef01"))]);
        let result = verdict(
            file()
                .with_crc32("abcdef01")
                .with_size(4096)
                .with_sha1(SHA1_B),
            &index,
        );
        assert!(
            matches!(result, AuditVerdict::Probable { .. }),
            "{result:?}"
        );
        assert!(
            !result.is_confident(),
            "CRC32 is never a confident identity"
        );
    }

    /// Malformed CRC32 text is not repaired. A short value is not left-padded
    /// with zeros into a match, on either side of the comparison.
    #[test]
    fn regression_short_or_malformed_crc_is_never_padded_into_a_match() {
        // File side: the DAT holds the full value; the caller holds junk.
        let index = index_of(vec![("Game A", rom("a.bin", 4096, "0bcdef01"))]);
        for malformed in [
            "bcdef01",
            "cdef01",
            "0x0bcdef01",
            "0bcdef0g",
            "00bcdef01",
            "",
            " ",
        ] {
            let result = verdict(file().with_crc32(malformed).with_size(4096), &index);
            assert_eq!(
                result,
                AuditVerdict::NoUsableEvidence,
                "{malformed:?} must not be looked up at all"
            );
        }
        // Case and surrounding whitespace are presentation, not content.
        for same_value in ["0BCDEF01", " 0bcdef01 "] {
            let result = verdict(file().with_crc32(same_value).with_size(4096), &index);
            assert!(
                matches!(result, AuditVerdict::Probable { .. }),
                "{same_value:?}"
            );
        }

        // DAT side, through the real parser: a seven-digit CRC is dropped as
        // malformed. It is neither indexed as written nor padded to "0bcdef01".
        let dir = tempfile::tempdir().unwrap();
        let dat_path = dir.path().join("short-crc.dat");
        std::fs::write(
            &dat_path,
            "<?xml version=\"1.0\"?>\n<datafile>\n<header><name>Regression</name></header>\n\
             <game name=\"Game A\"><description>Game A</description>\
             <rom name=\"a.bin\" size=\"4096\" crc=\"bcdef01\"/></game>\n</datafile>\n",
        )
        .unwrap();
        let parsed = crate::dat::parsers::parse_dat_file(&dat_path, Default::default())
            .unwrap()
            .dat;
        assert_eq!(parsed.games[0].roms[0].name, "a.bin");
        assert_eq!(parsed.games[0].roms[0].crc32, None, "not padded, not kept");
        let short = DatIndex::build(&parsed);
        assert_eq!(short.crc32_count(), 0);
        assert!(short.lookup_crc32("0bcdef01").is_empty());
        assert!(short.lookup_crc32("bcdef01").is_empty());
        let result = verdict(file().with_crc32("0bcdef01").with_size(4096), &short);
        assert_eq!(result, AuditVerdict::NotInDat);
    }

    #[test]
    fn cryptographic_matches_do_not_hide_other_crypto_contradictions() {
        let mut entry = rom("a.bin", 4096, "abcdef01");
        entry.sha256 = Some(SHA256_A.into());
        entry.sha1 = Some(SHA1_A.into());
        entry.md5 = Some(MD5_A.into());
        let index = index_of(vec![("A", entry)]);
        for known in [
            file().with_sha1(SHA1_A).with_md5(MD5_B),
            file().with_sha1(SHA1_B).with_md5(MD5_A),
            file().with_sha256(SHA256_A).with_sha1(SHA1_B),
            file().with_sha256(SHA256_B).with_sha1(SHA1_A),
            file()
                .with_crc32("abcdef01")
                .with_sha1(SHA1_A)
                .with_md5(MD5_B),
        ] {
            let actual = verdict(known, &index);
            assert!(
                matches!(&actual, AuditVerdict::Ambiguous { detail } if detail.contains("disagree")),
                "{actual:?}"
            );
            assert!(!actual.is_confident());
        }
    }

    #[test]
    fn agreement_missing_and_malformed_hashes_keep_existing_confidence() {
        let mut entry = rom("a.bin", 4096, "abcdef01");
        entry.sha1 = Some(SHA1_A.into());
        entry.md5 = Some(MD5_A.into());
        let index = index_of(vec![("A", entry)]);
        for known in [
            file()
                .with_sha1(SHA1_A)
                .with_md5(MD5_A)
                .with_crc32("abcdef01"),
            file().with_sha1(format!(" {} ", SHA1_A.to_uppercase())),
            file().with_sha1(SHA1_A).with_md5("malformed"),
            file().with_sha1(SHA1_A).with_sha256(SHA256_B), // DAT has no SHA-256.
        ] {
            assert!(verdict(known, &index).is_confident());
        }
        assert!(matches!(
            verdict(file().with_crc32("abcdef01").with_size(4096), &index),
            AuditVerdict::Probable { .. }
        ));
        assert_eq!(verdict(file(), &index), AuditVerdict::NoUsableEvidence);
    }

    #[test]
    fn contradictions_only_remove_their_own_strong_candidate() {
        let mut bad = rom("bad.bin", 4096, "abcdef01");
        bad.sha1 = Some(SHA1_A.into());
        bad.md5 = Some(MD5_A.into());
        let mut good = bad.clone();
        good.name = "good.bin".into();
        good.md5 = Some(MD5_B.into());
        let index = index_of(vec![("Bad", bad), ("Good", good)]);
        assert!(
            matches!(verdict(file().with_sha1(SHA1_A).with_md5(MD5_B), &index), AuditVerdict::Exact { game_name, .. } if game_name == "Good")
        );
    }

    #[test]
    fn contradicted_early_candidate_cannot_hide_valid_later_algorithm() {
        let mut bad = rom("bad.bin", 4096, "abcdef01");
        bad.sha1 = Some(SHA1_A.into());
        bad.md5 = Some(MD5_A.into());
        let mut good = rom("good.bin", 4096, "abcdef01");
        good.md5 = Some(MD5_B.into());
        let index = index_of(vec![("Bad", bad), ("Good", good)]);
        assert!(
            matches!(verdict(file().with_sha1(SHA1_A).with_md5(MD5_B), &index), AuditVerdict::Exact { game_name, algorithm: "MD5", .. } if game_name == "Good")
        );
    }
    // ---- Hard Job #6 / Operation Identity Lock regressions ----------------

    const SHA1_X: &str = "1111111111111111111111111111111111111111";
    const SHA1_Y: &str = "2222222222222222222222222222222222222222";
    const MD5_X: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const MD5_Z: &str = "cccccccccccccccccccccccccccccccc";

    fn rom_with(name: &str, sha1: Option<&str>, md5: Option<&str>) -> DatRomEntry {
        DatRomEntry {
            name: name.into(),
            size_bytes: Some(8),
            sha1: sha1.map(str::to_string),
            md5: md5.map(str::to_string),
            ..Default::default()
        }
    }

    fn file_with(sha1: Option<&str>, md5: Option<&str>) -> KnownFileEvidence {
        let mut known = file().with_size(8);
        if let Some(sha1) = sha1 {
            known = known.with_sha1(sha1);
        }
        if let Some(md5) = md5 {
            known = known.with_md5(md5);
        }
        known
    }

    /// S03-01 / D3-01: a second record with identical content that publishes
    /// only the weaker hash used to vanish behind the SHA-1 "unique" match.
    #[test]
    fn s03_01_identical_content_record_with_only_a_weaker_hash_is_not_hidden() {
        let index = index_of(vec![
            ("Game A", rom_with("a.bin", Some(SHA1_X), Some(MD5_X))),
            ("Game B", rom_with("b.bin", None, Some(MD5_X))),
        ]);
        match verdict(file_with(Some(SHA1_X), Some(MD5_X)), &index) {
            AuditVerdict::ExactMultipleCandidates {
                algorithm,
                count,
                game_names,
            } => {
                assert_eq!(algorithm, "SHA-1");
                assert_eq!(count, 2);
                assert_eq!(game_names, vec!["Game A", "Game B"]);
            }
            other => panic!("expected both candidates, got {other:?}"),
        }
    }

    /// The union is reported in catalogue order whichever algorithm found a
    /// record, so the verdict does not depend on publication choices.
    #[test]
    fn s03_01_candidate_order_is_catalogue_order_not_algorithm_order() {
        let index = index_of(vec![
            ("Game B", rom_with("b.bin", None, Some(MD5_X))),
            ("Game A", rom_with("a.bin", Some(SHA1_X), Some(MD5_X))),
        ]);
        match verdict(file_with(Some(SHA1_X), Some(MD5_X)), &index) {
            AuditVerdict::ExactMultipleCandidates { game_names, .. } => {
                assert_eq!(game_names, vec!["Game B", "Game A"]);
            }
            other => panic!("expected both candidates, got {other:?}"),
        }
        let collected = collect_crypto_candidates(&file_with(Some(SHA1_X), Some(MD5_X)), &index);
        assert_eq!(collected.consistent.len(), 2);
        assert_eq!(collected.algorithm, Some("SHA-1"));
        assert!(collected.unverifiable.is_empty());
        assert_eq!(collected.contradicted, 0);
    }

    /// A record that shares one hash but disagrees on another is a
    /// contradiction, never a second candidate.
    #[test]
    fn s03_01_sha1_and_md5_disagreement_excludes_the_record() {
        let index = index_of(vec![
            ("Game A", rom_with("a.bin", Some(SHA1_X), Some(MD5_X))),
            ("Game B", rom_with("b.bin", Some(SHA1_Y), Some(MD5_X))),
        ]);
        match verdict(file_with(Some(SHA1_X), Some(MD5_X)), &index) {
            AuditVerdict::Exact {
                game_name,
                algorithm,
                ..
            } => {
                assert_eq!(game_name, "Game A");
                assert_eq!(algorithm, "SHA-1");
            }
            other => panic!("expected exact Game A, got {other:?}"),
        }
        // The same file against a catalogue whose only record contradicts it.
        let only_conflict = index_of(vec![(
            "Game B",
            rom_with("b.bin", Some(SHA1_X), Some(MD5_Z)),
        )]);
        assert!(matches!(
            verdict(file_with(Some(SHA1_X), Some(MD5_X)), &only_conflict),
            AuditVerdict::Ambiguous { .. }
        ));
    }

    /// S03-03 / D3-03: a `baddump` record's hash agreeing with the file says
    /// "this is the known bad dump", never "this is the game".
    #[test]
    fn s03_03_a_baddump_record_never_becomes_an_exact_identity() {
        let mut bad = rom_with("game.bin", Some(SHA1_X), Some(MD5_X));
        bad.status = Some("baddump".into());
        let index = index_of(vec![("Game", bad)]);
        let report = audit_files(&[file_with(Some(SHA1_X), Some(MD5_X))], &index);
        match &report.entries[0].verdict {
            AuditVerdict::Ambiguous { detail } => {
                assert!(detail.contains("baddump"), "{detail}");
                assert!(detail.contains("Game / game.bin"), "{detail}");
            }
            other => panic!("expected an ambiguous refusal, got {other:?}"),
        }
        assert_eq!(report.summary.exact, 0);
        assert_eq!(report.summary.ambiguous, 1);
        assert!(!report.entries[0].verdict.is_confident());
    }

    /// A `nodump` record reached through CRC32 and size is not Probable either.
    #[test]
    fn s03_03_a_nodump_record_never_becomes_a_probable_identity() {
        let mut placeholder = rom("game.bin", 8, "deadbeef");
        placeholder.status = Some("NoDump".into());
        let index = index_of(vec![("Game", placeholder)]);
        match verdict(file().with_size(8).with_crc32("deadbeef"), &index) {
            AuditVerdict::Ambiguous { detail } => {
                assert!(detail.contains("nodump"), "{detail}");
            }
            other => panic!("expected an ambiguous refusal, got {other:?}"),
        }
    }

    /// A good record and a bad-dump record sharing a hash resolve to the good
    /// record alone; the bad one is excluded, not counted as a candidate.
    #[test]
    fn s03_03_a_good_record_is_not_made_ambiguous_by_a_baddump_twin() {
        let mut bad = rom_with("bad.bin", Some(SHA1_X), None);
        bad.status = Some("baddump".into());
        let index = index_of(vec![
            ("Game", rom_with("good.bin", Some(SHA1_X), None)),
            ("Game [b]", bad),
        ]);
        match verdict(file_with(Some(SHA1_X), None), &index) {
            AuditVerdict::Exact { game_name, .. } => assert_eq!(game_name, "Game"),
            other => panic!("expected exact Game, got {other:?}"),
        }
        let collected = collect_crypto_candidates(&file_with(Some(SHA1_X), None), &index);
        assert_eq!(collected.consistent.len(), 1);
        assert_eq!(collected.unverifiable.len(), 1);
        assert_eq!(collected.unverifiable[0].1, DumpStatus::BadDump);
    }

    /// `good`/`verified`/absent statuses stay ordinary; anything unknown is
    /// not treated as bad here (set classification owns that question).
    #[test]
    fn dump_status_recognises_only_nodump_and_baddump() {
        assert_eq!(DumpStatus::of(None), DumpStatus::Ordinary);
        assert_eq!(DumpStatus::of(Some("good")), DumpStatus::Ordinary);
        assert_eq!(DumpStatus::of(Some("verified")), DumpStatus::Ordinary);
        assert_eq!(DumpStatus::of(Some(" BADDUMP ")), DumpStatus::BadDump);
        assert_eq!(DumpStatus::of(Some("nodump")), DumpStatus::NoDump);
        assert_eq!(DumpStatus::of(Some("odd")), DumpStatus::Ordinary);
    }

    /// Identical names, different bytes: a hash that compares and misses is
    /// `NotInDat`; the shared filename never upgrades it.
    #[test]
    fn identical_names_with_different_bytes_are_not_in_dat() {
        let index = index_of(vec![("Game", rom_with("local.bin", Some(SHA1_X), None))]);
        assert_eq!(
            verdict(file_with(Some(SHA1_Y), None), &index),
            AuditVerdict::NotInDat
        );
    }
}
