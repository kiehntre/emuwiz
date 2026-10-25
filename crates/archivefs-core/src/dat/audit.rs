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
//! # Every shared algorithm is tried, not just the strongest
//!
//! A DAT carries the algorithms its publisher chose; the caller knows whatever
//! EmuWiz happens to have computed. The two sets overlap but neither contains
//! the other - a No-Intro DAT publishes CRC32/MD5/SHA-1 and no SHA-256, so
//! stopping at the strongest hash the *caller* holds reports a perfectly matching
//! file as absent from the catalogue. Each algorithm is tried in descending order
//! of strength. Only candidates whose shared, well-formed cryptographic hashes
//! agree may decide a match; a CRC32 match cannot override such a contradiction.
//! Missing algorithms are not contradictions. This compares supplied evidence,
//! not newly read file content, and retains the existing confidence tiers.

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

pub(crate) fn audit_one(known: &KnownFileEvidence, index: &DatIndex) -> AuditVerdict {
    // Strongest first, but *every* algorithm the caller holds is tried until one
    // finds candidates. Returning on the first algorithm the caller happens to
    // know reports a matching file as absent whenever the DAT does not publish
    // that particular algorithm.
    let mut compared_any = false;
    let mut crypto_conflict = false;

    for (value, algorithm) in [
        (known.sha256.as_deref(), "SHA-256"),
        (known.sha1.as_deref(), "SHA-1"),
        (known.md5.as_deref(), "MD5"),
    ] {
        let Some(value) = value else { continue };
        let Some(normalised) = normalise_for_lookup(value, algorithm) else {
            continue;
        };
        compared_any = true;
        let candidates = match algorithm {
            "SHA-256" => index.lookup_sha256(&normalised),
            "SHA-1" => index.lookup_sha1(&normalised),
            _ => index.lookup_md5(&normalised),
        };
        if !candidates.is_empty() {
            let consistent: Vec<_> = candidates
                .iter()
                .filter(|candidate| !cryptographic_hash_disagrees(known, candidate))
                .cloned()
                .collect();
            if !consistent.is_empty() {
                return handle_candidates(&consistent, algorithm);
            }
            crypto_conflict = true;
        }
    }

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
}
