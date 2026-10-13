//! DAT Duel: read-only comparison of two parsed DAT catalogues.
//!
//! Answers "where do these two catalogues agree or disagree about ROM
//! identity?" without importing either into the database, selecting an
//! authority, or touching any ROM. It is deliberately not
//! [`super::divergence`] (one DAT's metadata against its own filename) and not
//! [`super::authority`] (retained inventories): it compares the complete
//! per-ROM hash, size, name and BIOS content of two catalogues.
//!
//! Identity rules, strongest first:
//! * **Verified**: both records carry the same SHA-256, SHA-1 or MD5, with no
//!   other recorded hash or size contradicting it.
//! * **Probable**: CRC32 and size agree, no strong hash is shared, and no
//!   recorded field contradicts. CRC32+size is a weak identity; it is never
//!   reported as verified.
//! * A name alone is never identity. Equal names with no comparable data are
//!   reported as unverified, not as agreement.
//!
//! The comparison is a pure function over [`ParsedDat`] values, so the existing
//! parsers and their [`super::limits::DatLimits`] bound memory and input size.
//! Candidate lists and report sections are additionally capped here.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::model::{
    ChecksumAlgorithm, DatChecksum, DatEcosystem, DatFormat, DatRomEntry, DatSource, ParsedDat,
};

pub const REPORT_SCHEMA_VERSION: u32 = 1;
/// Per-record cap on hash-bucket candidates inspected. Larger buckets are
/// reported as truncated rather than expanded quadratically.
const MAX_CANDIDATES_PER_RECORD: usize = 8;
/// Cap on records listed inside one ambiguity group.
const MAX_GROUP_MEMBERS: usize = 64;
/// Cap on names listed for one duplicate-checksum group.
const MAX_DUPLICATE_NAMES: usize = 8;
pub const DEFAULT_MAX_ITEMS_PER_SECTION: usize = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Strength {
    None,
    Crc32Only,
    Crc32AndSize,
    Md5,
    Sha1,
    Sha256,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RomEvidence {
    pub game: String,
    pub rom: String,
    /// `rom` or `disk`.
    pub kind: &'static str,
    pub size_bytes: Option<u64>,
    pub crc32: Option<String>,
    pub md5: Option<String>,
    pub sha1: Option<String>,
    pub sha256: Option<String>,
    pub strength: Strength,
    pub bios: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Compatibility {
    Compatible,
    /// Same ecosystem and platform base name, differing variant tags such as
    /// `(Headered)` / `(Headerless)`. Hash disagreement is expected.
    VariantDiffers,
    /// A header name is missing, so the namespace cannot be confirmed.
    UnverifiedNamespace,
    IncompatibleEcosystem,
    IncompatibleCatalogue,
}

impl Compatibility {
    fn is_incompatible(self) -> bool {
        matches!(
            self,
            Self::IncompatibleEcosystem | Self::IncompatibleCatalogue
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NameRelation {
    Identical,
    CaseOnly,
    SameGameDifferentRom,
    DifferentGameSameRom,
    Different,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Verified,
    Probable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MatchBasis {
    /// `sha256`, `sha1`, `md5` or `crc32_size`.
    pub basis: &'static str,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Contradiction {
    pub field: &'static str,
    pub a: String,
    pub b: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pairing {
    pub a: RomEvidence,
    pub b: RomEvidence,
    pub name_relation: NameRelation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_by: Option<MatchBasis>,
    pub contradictions: Vec<Contradiction>,
    pub bios_differs: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AmbiguousGroup {
    pub a: Vec<RomEvidence>,
    pub b: Vec<RomEvidence>,
    /// Every candidate relation inside the group, strongest basis recorded.
    pub links: Vec<GroupLink>,
    pub members_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GroupLink {
    pub a: String,
    pub b: String,
    pub matched_by: MatchBasis,
    pub name_relation: NameRelation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Absence {
    /// The record has a usable checksum and nothing in the other catalogue
    /// matches it by checksum or by name.
    NoCounterpart,
    /// The record has no usable checksum (or CRC32 without size), so absence
    /// is not a confirmed difference.
    Unverifiable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OnlyEntry {
    pub record: RomEvidence,
    pub absence: Absence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DuplicateChecksumGroup {
    pub basis: &'static str,
    pub value: String,
    pub record_count: usize,
    pub names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileFingerprint {
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceSummary {
    pub file_path: String,
    pub format: DatFormat,
    pub ecosystem: DatEcosystem,
    pub ecosystem_label: &'static str,
    pub name: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
    pub author: Option<String>,
    pub homepage: Option<String>,
    pub entry_count: usize,
    pub rom_count: usize,
    pub parse_warning_count: usize,
    pub records_compared: usize,
    pub invalid_checksum_fields: usize,
    pub file: Option<FileFingerprint>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub verified_same_name: usize,
    pub probable_same_name: usize,
    pub renamed_verified: usize,
    pub renamed_probable: usize,
    pub verified_by_sha256: usize,
    pub verified_by_sha1: usize,
    pub verified_by_md5: usize,
    pub ambiguous_groups: usize,
    pub same_name_conflicts: usize,
    pub cross_name_conflicts: usize,
    pub same_name_unverified: usize,
    pub only_in_a_no_counterpart: usize,
    pub only_in_a_unverifiable: usize,
    pub only_in_b_no_counterpart: usize,
    pub only_in_b_unverifiable: usize,
    pub duplicate_checksum_groups_a: usize,
    pub duplicate_checksum_groups_b: usize,
    pub bios_flag_differences: usize,
    pub candidate_buckets_truncated: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DuelReport {
    pub schema_version: u32,
    pub read_only: bool,
    pub compatibility: Compatibility,
    /// False when the catalogues are in different namespaces and the caller
    /// explicitly asked to continue: name differences are then not meaningful.
    pub naming_comparable: bool,
    pub sources: [SourceSummary; 2],
    pub summary: Summary,
    pub renamed: Vec<Pairing>,
    pub same_name_conflicts: Vec<Pairing>,
    pub cross_name_conflicts: Vec<Pairing>,
    pub same_name_unverified: Vec<Pairing>,
    pub ambiguous: Vec<AmbiguousGroup>,
    pub only_in_a: Vec<OnlyEntry>,
    pub only_in_b: Vec<OnlyEntry>,
    pub duplicate_checksums_a: Vec<DuplicateChecksumGroup>,
    pub duplicate_checksums_b: Vec<DuplicateChecksumGroup>,
    /// Sections whose listing was cut at `max_items_per_section`; counts in
    /// `summary` are always complete.
    pub truncated_sections: Vec<&'static str>,
}

#[derive(Debug, Clone, Copy)]
pub struct DuelOptions {
    pub allow_incompatible: bool,
    pub max_items_per_section: usize,
}

impl Default for DuelOptions {
    fn default() -> Self {
        Self {
            allow_incompatible: false,
            max_items_per_section: DEFAULT_MAX_ITEMS_PER_SECTION,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuelRefusal {
    pub compatibility: Compatibility,
    pub detail: String,
}

impl std::fmt::Display for DuelRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for DuelRefusal {}

/// Streams `path` through SHA-256. Read-only; provenance for a report.
pub fn fingerprint_file(path: &Path) -> std::io::Result<FileFingerprint> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    Ok(FileFingerprint {
        sha256: digest.iter().map(|b| format!("{b:02x}")).collect(),
        size_bytes: size,
    })
}

fn platform_base(name: &str) -> String {
    name.split(" (")
        .next()
        .unwrap_or(name)
        .trim()
        .to_ascii_lowercase()
}

pub fn assess_namespaces(a: &DatSource, b: &DatSource) -> Compatibility {
    if a.ecosystem != b.ecosystem {
        return Compatibility::IncompatibleEcosystem;
    }
    match (a.name.as_deref(), b.name.as_deref()) {
        (Some(x), Some(y)) => {
            if platform_base(x) != platform_base(y) {
                Compatibility::IncompatibleCatalogue
            } else if x.trim() == y.trim() {
                Compatibility::Compatible
            } else {
                Compatibility::VariantDiffers
            }
        }
        _ => Compatibility::UnverifiedNamespace,
    }
}

#[derive(Debug, Clone)]
struct Rec {
    ev: RomEvidence,
}

fn truthy(value: Option<&str>) -> bool {
    matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("yes" | "true" | "1")
    )
}

fn norm(algorithm: ChecksumAlgorithm, raw: &Option<String>, invalid: &mut usize) -> Option<String> {
    let raw = raw.as_deref()?;
    match DatChecksum::parse(algorithm, raw) {
        Some(c) => Some(c.value),
        None => {
            *invalid += 1;
            None
        }
    }
}

fn strength_of(ev: &RomEvidence) -> Strength {
    if ev.sha256.is_some() {
        Strength::Sha256
    } else if ev.sha1.is_some() {
        Strength::Sha1
    } else if ev.md5.is_some() {
        Strength::Md5
    } else if ev.crc32.is_some() && ev.size_bytes.is_some() {
        Strength::Crc32AndSize
    } else if ev.crc32.is_some() {
        Strength::Crc32Only
    } else {
        Strength::None
    }
}

fn push_rom(out: &mut Vec<Rec>, invalid: &mut usize, game: &str, bios: bool, rom: &DatRomEntry) {
    let mut ev = RomEvidence {
        game: game.to_string(),
        rom: rom.name.clone(),
        kind: "rom",
        size_bytes: rom.size_bytes,
        crc32: norm(ChecksumAlgorithm::Crc32, &rom.crc32, invalid),
        md5: norm(ChecksumAlgorithm::Md5, &rom.md5, invalid),
        sha1: norm(ChecksumAlgorithm::Sha1, &rom.sha1, invalid),
        sha256: norm(ChecksumAlgorithm::Sha256, &rom.sha256, invalid),
        strength: Strength::None,
        bios: bios || truthy(rom.bios.as_deref()),
    };
    ev.strength = strength_of(&ev);
    out.push(Rec { ev });
}

fn push_disk(
    out: &mut Vec<Rec>,
    invalid: &mut usize,
    game: &str,
    bios: bool,
    disk: &super::model::DatDiskEntry,
) {
    let mut ev = RomEvidence {
        game: game.to_string(),
        rom: disk.name.clone().unwrap_or_default(),
        kind: "disk",
        size_bytes: None,
        crc32: None,
        md5: None,
        sha1: norm(ChecksumAlgorithm::Sha1, &disk.sha1, invalid),
        sha256: None,
        strength: Strength::None,
        bios,
    };
    ev.strength = strength_of(&ev);
    out.push(Rec { ev });
}

fn collect(dat: &ParsedDat) -> (Vec<Rec>, usize) {
    let mut out = Vec::new();
    let mut invalid = 0usize;
    for game in &dat.games {
        let bios = truthy(game.is_bios.as_deref());
        for rom in &game.roms {
            push_rom(&mut out, &mut invalid, &game.name, bios, rom);
        }
        for disk in &game.disks {
            push_disk(&mut out, &mut invalid, &game.name, bios, disk);
        }
        for part in &game.parts {
            for area in &part.data_areas {
                for rom in &area.roms {
                    push_rom(&mut out, &mut invalid, &game.name, bios, rom);
                }
            }
            for area in &part.disk_areas {
                for disk in &area.disks {
                    push_disk(&mut out, &mut invalid, &game.name, bios, disk);
                }
            }
        }
    }
    (out, invalid)
}

#[derive(Default)]
struct Index {
    by_hash: HashMap<(&'static str, String), Vec<usize>>,
    by_crc_size: HashMap<(String, u64), Vec<usize>>,
    by_name: HashMap<(String, String, &'static str), Vec<usize>>,
}

fn build_index(recs: &[Rec]) -> Index {
    let mut idx = Index::default();
    for (i, r) in recs.iter().enumerate() {
        let e = &r.ev;
        for (tag, value) in [("sha256", &e.sha256), ("sha1", &e.sha1), ("md5", &e.md5)] {
            if let Some(v) = value {
                idx.by_hash.entry((tag, v.clone())).or_default().push(i);
            }
        }
        if let (Some(c), Some(s)) = (&e.crc32, e.size_bytes) {
            idx.by_crc_size.entry((c.clone(), s)).or_default().push(i);
        }
        idx.by_name
            .entry((e.game.clone(), e.rom.clone(), e.kind))
            .or_default()
            .push(i);
    }
    idx
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tier {
    Verified(&'static str),
    Probable,
    Conflict,
    Unrelated,
}

fn contradictions(a: &RomEvidence, b: &RomEvidence) -> (Vec<Contradiction>, Vec<&'static str>) {
    let mut contra = Vec::new();
    let mut agree = Vec::new();
    let mut cmp = |field: &'static str, x: &Option<String>, y: &Option<String>| {
        if let (Some(x), Some(y)) = (x, y) {
            if x == y {
                agree.push(field);
            } else {
                contra.push(Contradiction {
                    field,
                    a: x.clone(),
                    b: y.clone(),
                });
            }
        }
    };
    cmp("sha256", &a.sha256, &b.sha256);
    cmp("sha1", &a.sha1, &b.sha1);
    cmp("md5", &a.md5, &b.md5);
    cmp("crc32", &a.crc32, &b.crc32);
    if let (Some(x), Some(y)) = (a.size_bytes, b.size_bytes)
        && x != y
    {
        contra.push(Contradiction {
            field: "size_bytes",
            a: x.to_string(),
            b: y.to_string(),
        });
    }
    (contra, agree)
}

fn tier(a: &RomEvidence, b: &RomEvidence) -> (Tier, Vec<Contradiction>) {
    if a.kind != b.kind {
        return (Tier::Unrelated, Vec::new());
    }
    let (contra, agree) = contradictions(a, b);
    let strong_agree = ["sha256", "sha1", "md5"]
        .into_iter()
        .find(|f| agree.contains(f));
    let strong_disagree = contra
        .iter()
        .any(|c| matches!(c.field, "sha256" | "sha1" | "md5"));
    let crc_size_equal = agree.contains(&"crc32")
        && matches!((a.size_bytes, b.size_bytes), (Some(x), Some(y)) if x == y);
    let t = match strong_agree {
        Some(f) if contra.is_empty() => Tier::Verified(f),
        Some(_) => Tier::Conflict,
        None if strong_disagree && crc_size_equal => Tier::Conflict,
        None if strong_disagree => Tier::Unrelated,
        None if crc_size_equal && contra.is_empty() => Tier::Probable,
        None => Tier::Unrelated,
    };
    (t, contra)
}

fn name_relation(a: &RomEvidence, b: &RomEvidence) -> NameRelation {
    if a.game == b.game && a.rom == b.rom {
        NameRelation::Identical
    } else if a.game.eq_ignore_ascii_case(&b.game) && a.rom.eq_ignore_ascii_case(&b.rom) {
        NameRelation::CaseOnly
    } else if a.game == b.game {
        NameRelation::SameGameDifferentRom
    } else if a.rom == b.rom {
        NameRelation::DifferentGameSameRom
    } else {
        NameRelation::Different
    }
}

fn basis_of(t: &Tier) -> Option<MatchBasis> {
    match t {
        Tier::Verified(f) => Some(MatchBasis {
            basis: f,
            confidence: Confidence::Verified,
        }),
        Tier::Probable => Some(MatchBasis {
            basis: "crc32_size",
            confidence: Confidence::Probable,
        }),
        _ => None,
    }
}

fn candidates(rec: &RomEvidence, other: &Index, truncated: &mut usize) -> BTreeSet<usize> {
    let mut found = BTreeSet::new();
    let mut take = |bucket: Option<&Vec<usize>>| {
        if let Some(bucket) = bucket {
            if bucket.len() > MAX_CANDIDATES_PER_RECORD {
                *truncated += 1;
            }
            found.extend(bucket.iter().take(MAX_CANDIDATES_PER_RECORD).copied());
        }
    };
    for (tag, value) in [
        ("sha256", &rec.sha256),
        ("sha1", &rec.sha1),
        ("md5", &rec.md5),
    ] {
        if let Some(v) = value {
            take(other.by_hash.get(&(tag, v.clone())));
        }
    }
    if let (Some(c), Some(s)) = (&rec.crc32, rec.size_bytes) {
        take(other.by_crc_size.get(&(c.clone(), s)));
    }
    found
}

struct Dsu(Vec<usize>);
impl Dsu {
    fn find(&mut self, x: usize) -> usize {
        let mut r = x;
        while self.0[r] != r {
            r = self.0[r];
        }
        let mut c = x;
        while self.0[c] != r {
            let n = self.0[c];
            self.0[c] = r;
            c = n;
        }
        r
    }
    fn union(&mut self, a: usize, b: usize) {
        let (x, y) = (self.find(a), self.find(b));
        if x != y {
            self.0[x.max(y)] = x.min(y);
        }
    }
}

fn duplicates(recs: &[Rec]) -> Vec<DuplicateChecksumGroup> {
    let mut groups: BTreeMap<(&'static str, String), BTreeSet<String>> = BTreeMap::new();
    let mut counts: BTreeMap<(&'static str, String), usize> = BTreeMap::new();
    for r in recs {
        let e = &r.ev;
        let key = if let Some(v) = &e.sha256 {
            Some(("sha256", v.clone()))
        } else if let Some(v) = &e.sha1 {
            Some(("sha1", v.clone()))
        } else {
            e.md5.as_ref().map(|v| ("md5", v.clone()))
        };
        if let Some(key) = key {
            *counts.entry(key.clone()).or_default() += 1;
            groups
                .entry(key)
                .or_default()
                .insert(format!("{}/{}", e.game, e.rom));
        }
    }
    groups
        .into_iter()
        .filter(|(_, names)| names.len() > 1)
        .map(|(key, names)| DuplicateChecksumGroup {
            basis: key.0,
            record_count: counts[&key],
            value: key.1,
            names: names.into_iter().take(MAX_DUPLICATE_NAMES).collect(),
        })
        .collect()
}

fn summarise(
    dat: &ParsedDat,
    recs: &[Rec],
    invalid: usize,
    file: Option<FileFingerprint>,
) -> SourceSummary {
    let s = &dat.source;
    SourceSummary {
        file_path: s.file_path.clone(),
        format: s.format,
        ecosystem: s.ecosystem,
        ecosystem_label: s.ecosystem.label(),
        name: s.name.clone(),
        description: s.description.clone(),
        version: s.version.clone(),
        author: s.author.clone(),
        homepage: s.homepage.clone(),
        entry_count: s.entry_count,
        rom_count: s.rom_count,
        parse_warning_count: s.parse_warnings.len(),
        records_compared: recs.len(),
        invalid_checksum_fields: invalid,
        file,
    }
}

fn sort_key(e: &RomEvidence) -> (String, String, &'static str) {
    (e.game.clone(), e.rom.clone(), e.kind)
}

fn capped<T>(
    mut items: Vec<T>,
    limit: usize,
    name: &'static str,
    cut: &mut Vec<&'static str>,
) -> Vec<T> {
    if items.len() > limit {
        items.truncate(limit);
        cut.push(name);
    }
    items
}

/// Compares two parsed catalogues. `files` carries optional provenance
/// fingerprints for A and B.
pub fn compare(
    a: &ParsedDat,
    b: &ParsedDat,
    files: [Option<FileFingerprint>; 2],
    options: DuelOptions,
) -> Result<DuelReport, DuelRefusal> {
    let compatibility = assess_namespaces(&a.source, &b.source);
    if compatibility.is_incompatible() && !options.allow_incompatible {
        return Err(DuelRefusal {
            compatibility,
            detail: format!(
                "refusing to compare incompatible catalogues: A is {:?} / {:?}, B is {:?} / {:?}. \
                 Different ecosystems or platform catalogues are not interchangeable; pass \
                 --allow-incompatible to see hash evidence only.",
                a.source.ecosystem.label(),
                a.source.name,
                b.source.ecosystem.label(),
                b.source.name
            ),
        });
    }

    let (ra, inv_a) = collect(a);
    let (rb, inv_b) = collect(b);
    let (ia, ib) = (build_index(&ra), build_index(&rb));
    let mut summary = Summary::default();

    // Candidate links, discovered from both sides so a capped bucket on one
    // side cannot hide a relation the other side sees.
    let mut pairs: BTreeSet<(usize, usize)> = BTreeSet::new();
    for (i, r) in ra.iter().enumerate() {
        for j in candidates(&r.ev, &ib, &mut summary.candidate_buckets_truncated) {
            pairs.insert((i, j));
        }
    }
    for (j, r) in rb.iter().enumerate() {
        for i in candidates(&r.ev, &ia, &mut summary.candidate_buckets_truncated) {
            pairs.insert((i, j));
        }
    }

    let mut matched: Vec<(usize, usize, Tier)> = Vec::new();
    let mut conflicts: BTreeMap<(usize, usize), Vec<Contradiction>> = BTreeMap::new();
    for &(i, j) in &pairs {
        let (t, contra) = tier(&ra[i].ev, &rb[j].ev);
        match t {
            Tier::Verified(_) | Tier::Probable => matched.push((i, j, t)),
            Tier::Conflict => {
                conflicts.insert((i, j), contra);
            }
            Tier::Unrelated => {}
        }
    }

    let (na, nb) = (ra.len(), rb.len());
    let mut dsu = Dsu((0..na + nb).collect());
    for (i, j, _) in &matched {
        dsu.union(*i, na + *j);
    }
    let mut comps: BTreeMap<usize, (Vec<usize>, Vec<usize>)> = BTreeMap::new();
    let mut matched_a = vec![false; na];
    let mut matched_b = vec![false; nb];
    for (i, j, _) in &matched {
        matched_a[*i] = true;
        matched_b[*j] = true;
    }
    for (i, _) in matched_a.iter().enumerate().filter(|(_, m)| **m) {
        let r = dsu.find(i);
        comps.entry(r).or_default().0.push(i);
    }
    for (j, _) in matched_b.iter().enumerate().filter(|(_, m)| **m) {
        let r = dsu.find(na + j);
        comps.entry(r).or_default().1.push(j);
    }
    let tier_of: BTreeMap<(usize, usize), Tier> = matched
        .iter()
        .map(|(i, j, t)| ((*i, *j), t.clone()))
        .collect();

    let make_pairing =
        |i: usize, j: usize, contra: Vec<Contradiction>, t: Option<&Tier>| -> Pairing {
            let (ea, eb) = (&ra[i].ev, &rb[j].ev);
            Pairing {
                a: ea.clone(),
                b: eb.clone(),
                name_relation: name_relation(ea, eb),
                matched_by: t.and_then(basis_of),
                contradictions: contra,
                bios_differs: ea.bios != eb.bios,
            }
        };

    let mut renamed = Vec::new();
    let mut ambiguous = Vec::new();
    for (_, (ai, bj)) in comps {
        // Perfect pairing by identical names (MAME-style parent/clone repeats)
        // is agreement, not ambiguity.
        let ident = |i: usize, j: usize| {
            tier_of.contains_key(&(i, j))
                && name_relation(&ra[i].ev, &rb[j].ev) == NameRelation::Identical
        };
        let all_paired = ai.iter().all(|&i| bj.iter().any(|&j| ident(i, j)))
            && bj.iter().all(|&j| ai.iter().any(|&i| ident(i, j)));
        if ai.len() == 1 && bj.len() == 1 {
            let t = tier_of[&(ai[0], bj[0])].clone();
            let p = make_pairing(ai[0], bj[0], Vec::new(), Some(&t));
            if p.bios_differs {
                summary.bios_flag_differences += 1;
            }
            if let Tier::Verified(f) = &t {
                match *f {
                    "sha256" => summary.verified_by_sha256 += 1,
                    "sha1" => summary.verified_by_sha1 += 1,
                    _ => summary.verified_by_md5 += 1,
                }
            }
            let verified = matches!(t, Tier::Verified(_));
            if p.name_relation == NameRelation::Identical {
                if verified {
                    summary.verified_same_name += 1;
                } else {
                    summary.probable_same_name += 1;
                }
                if p.bios_differs {
                    // surfaced as a rename-style finding so it stays visible
                    renamed.push(p);
                }
            } else {
                if verified {
                    summary.renamed_verified += 1;
                } else {
                    summary.renamed_probable += 1;
                }
                renamed.push(p);
            }
        } else if all_paired {
            summary.verified_same_name += ai.len();
        } else {
            summary.ambiguous_groups += 1;
            let truncated = ai.len() > MAX_GROUP_MEMBERS || bj.len() > MAX_GROUP_MEMBERS;
            let mut links = Vec::new();
            for &i in ai.iter().take(MAX_GROUP_MEMBERS) {
                for &j in bj.iter().take(MAX_GROUP_MEMBERS) {
                    if let Some(t) = tier_of.get(&(i, j)) {
                        links.push(GroupLink {
                            a: format!("{}/{}", ra[i].ev.game, ra[i].ev.rom),
                            b: format!("{}/{}", rb[j].ev.game, rb[j].ev.rom),
                            matched_by: basis_of(t).expect("matched tier has a basis"),
                            name_relation: name_relation(&ra[i].ev, &rb[j].ev),
                        });
                    }
                }
            }
            ambiguous.push(AmbiguousGroup {
                a: ai
                    .iter()
                    .take(MAX_GROUP_MEMBERS)
                    .map(|&i| ra[i].ev.clone())
                    .collect(),
                b: bj
                    .iter()
                    .take(MAX_GROUP_MEMBERS)
                    .map(|&j| rb[j].ev.clone())
                    .collect(),
                links,
                members_truncated: truncated,
            });
        }
    }

    // Same-name relations for records without a hash match, in both directions.
    let mut handled: BTreeSet<(usize, usize)> = BTreeSet::new();
    let mut same_name_conflicts = Vec::new();
    let mut cross_name_conflicts = Vec::new();
    let mut same_name_unverified = Vec::new();
    let mut named_a = vec![false; na];
    let mut named_b = vec![false; nb];

    for (&(i, j), contra) in &conflicts {
        handled.insert((i, j));
        named_a[i] = true;
        named_b[j] = true;
        let p = make_pairing(i, j, contra.clone(), None);
        if p.name_relation == NameRelation::Identical {
            same_name_conflicts.push(p);
        } else {
            cross_name_conflicts.push(p);
        }
    }

    let mut same_name = |i: usize,
                         j: usize,
                         handled: &mut BTreeSet<(usize, usize)>,
                         named_a: &mut Vec<bool>,
                         named_b: &mut Vec<bool>| {
        if !handled.insert((i, j)) {
            return;
        }
        named_a[i] = true;
        named_b[j] = true;
        let (_, contra) = tier(&ra[i].ev, &rb[j].ev);
        let p = make_pairing(i, j, contra.clone(), None);
        if contra.is_empty() {
            same_name_unverified.push(p);
        } else {
            same_name_conflicts.push(p);
        }
    };
    for i in 0..na {
        if matched_a[i] {
            continue;
        }
        let e = &ra[i].ev;
        if let Some(js) = ib.by_name.get(&(e.game.clone(), e.rom.clone(), e.kind)) {
            for &j in js.iter().take(MAX_CANDIDATES_PER_RECORD) {
                same_name(i, j, &mut handled, &mut named_a, &mut named_b);
            }
        }
    }
    for j in 0..nb {
        if matched_b[j] {
            continue;
        }
        let e = &rb[j].ev;
        if let Some(is) = ia.by_name.get(&(e.game.clone(), e.rom.clone(), e.kind)) {
            for &i in is.iter().take(MAX_CANDIDATES_PER_RECORD) {
                same_name(i, j, &mut handled, &mut named_a, &mut named_b);
            }
        }
    }
    // A matched b whose same-named a matched something else is a conflict from
    // the unmatched side's view; covered above because only unmatched records
    // seed the loops and `j`/`i` may be matched.

    let only = |recs: &[Rec], matched: &[bool], named: &[bool]| -> Vec<OnlyEntry> {
        let mut out: Vec<OnlyEntry> = recs
            .iter()
            .enumerate()
            .filter(|(i, _)| !matched[*i] && !named[*i])
            .map(|(_, r)| OnlyEntry {
                record: r.ev.clone(),
                absence: if r.ev.strength >= Strength::Crc32AndSize {
                    Absence::NoCounterpart
                } else {
                    Absence::Unverifiable
                },
            })
            .collect();
        out.sort_by_key(|e| sort_key(&e.record));
        out
    };
    let only_in_a = only(&ra, &matched_a, &named_a);
    let only_in_b = only(&rb, &matched_b, &named_b);
    for e in &only_in_a {
        match e.absence {
            Absence::NoCounterpart => summary.only_in_a_no_counterpart += 1,
            Absence::Unverifiable => summary.only_in_a_unverifiable += 1,
        }
    }
    for e in &only_in_b {
        match e.absence {
            Absence::NoCounterpart => summary.only_in_b_no_counterpart += 1,
            Absence::Unverifiable => summary.only_in_b_unverifiable += 1,
        }
    }

    let pair_key = |p: &Pairing| (sort_key(&p.a), sort_key(&p.b));
    renamed.sort_by_key(pair_key);
    same_name_conflicts.sort_by_key(pair_key);
    cross_name_conflicts.sort_by_key(pair_key);
    same_name_unverified.sort_by_key(pair_key);
    ambiguous.sort_by_key(|g| (g.a.first().map(sort_key), g.b.first().map(sort_key)));

    summary.same_name_conflicts = same_name_conflicts.len();
    summary.cross_name_conflicts = cross_name_conflicts.len();
    summary.same_name_unverified = same_name_unverified.len();

    let dup_a = duplicates(&ra);
    let dup_b = duplicates(&rb);
    summary.duplicate_checksum_groups_a = dup_a.len();
    summary.duplicate_checksum_groups_b = dup_b.len();

    let limit = options.max_items_per_section;
    let mut cut = Vec::new();
    let [fa, fb] = files;
    Ok(DuelReport {
        schema_version: REPORT_SCHEMA_VERSION,
        read_only: true,
        compatibility,
        naming_comparable: !compatibility.is_incompatible(),
        sources: [summarise(a, &ra, inv_a, fa), summarise(b, &rb, inv_b, fb)],
        summary,
        renamed: capped(renamed, limit, "renamed", &mut cut),
        same_name_conflicts: capped(same_name_conflicts, limit, "same_name_conflicts", &mut cut),
        cross_name_conflicts: capped(
            cross_name_conflicts,
            limit,
            "cross_name_conflicts",
            &mut cut,
        ),
        same_name_unverified: capped(
            same_name_unverified,
            limit,
            "same_name_unverified",
            &mut cut,
        ),
        ambiguous: capped(ambiguous, limit, "ambiguous", &mut cut),
        only_in_a: capped(only_in_a, limit, "only_in_a", &mut cut),
        only_in_b: capped(only_in_b, limit, "only_in_b", &mut cut),
        duplicate_checksums_a: capped(dup_a, limit, "duplicate_checksums_a", &mut cut),
        duplicate_checksums_b: capped(dup_b, limit, "duplicate_checksums_b", &mut cut),
        truncated_sections: cut,
    })
}

fn label(e: &RomEvidence) -> String {
    format!("{} / {}", e.game, e.rom)
}

/// Human-readable rendering; at most `per_section` entries per section.
pub fn render_text(report: &DuelReport, per_section: usize) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(out, "DAT Duel (read-only)");
    for (tag, s) in ["A", "B"].iter().zip(report.sources.iter()) {
        let _ = writeln!(
            out,
            "  {tag}: {} [{} / {}] name={:?} version={:?} author={:?}\n     {} entries, {} ROM records compared, {} parse warnings{}",
            s.file_path,
            s.ecosystem_label,
            s.format.label(),
            s.name,
            s.version,
            s.author,
            s.entry_count,
            s.records_compared,
            s.parse_warning_count,
            s.file
                .as_ref()
                .map(|f| format!(", sha256 {}", f.sha256))
                .unwrap_or_default()
        );
    }
    let _ = writeln!(out, "Namespace: {:?}", report.compatibility);
    if !report.naming_comparable {
        let _ = writeln!(
            out,
            "  WARNING: catalogues are not interchangeable; name differences below are not meaningful."
        );
    }
    let s = &report.summary;
    let _ = writeln!(out, "Summary:");
    for (k, v) in [
        ("agree, verified, same name", s.verified_same_name),
        (
            "agree, probable (CRC32+size), same name",
            s.probable_same_name,
        ),
        (
            "identical hash, different name (verified)",
            s.renamed_verified,
        ),
        (
            "identical CRC32+size, different name (probable)",
            s.renamed_probable,
        ),
        ("ambiguous / duplicate matches", s.ambiguous_groups),
        ("same name, conflicting hash or size", s.same_name_conflicts),
        (
            "different name, contradictory evidence",
            s.cross_name_conflicts,
        ),
        (
            "same name, no comparable checksum (unverified)",
            s.same_name_unverified,
        ),
        ("only in A (no counterpart)", s.only_in_a_no_counterpart),
        (
            "only in A (unverifiable: missing checksums)",
            s.only_in_a_unverifiable,
        ),
        ("only in B (no counterpart)", s.only_in_b_no_counterpart),
        (
            "only in B (unverifiable: missing checksums)",
            s.only_in_b_unverifiable,
        ),
        ("BIOS flag differences", s.bios_flag_differences),
    ] {
        let _ = writeln!(out, "  {v:>8}  {k}");
    }
    let pairs = |out: &mut String, title: &str, items: &[Pairing]| {
        if items.is_empty() {
            return;
        }
        let _ = writeln!(out, "\n{title} ({}):", items.len());
        for p in items.iter().take(per_section) {
            let basis = p
                .matched_by
                .as_ref()
                .map(|m| format!("{:?} by {}", m.confidence, m.basis))
                .unwrap_or_else(|| "no match".into());
            let _ = writeln!(
                out,
                "  A: {}\n  B: {}\n     [{basis}; names {:?}]",
                label(&p.a),
                label(&p.b),
                p.name_relation
            );
            for c in &p.contradictions {
                let _ = writeln!(out, "     {} differs: A={} B={}", c.field, c.a, c.b);
            }
            if p.bios_differs {
                let _ = writeln!(
                    out,
                    "     BIOS flag differs (A={}, B={})",
                    p.a.bios, p.b.bios
                );
            }
        }
    };
    pairs(
        &mut out,
        "Identical content, different names",
        &report.renamed,
    );
    pairs(
        &mut out,
        "Same name, conflicting evidence",
        &report.same_name_conflicts,
    );
    pairs(
        &mut out,
        "Different names, contradictory evidence",
        &report.cross_name_conflicts,
    );
    pairs(
        &mut out,
        "Same name, unverified",
        &report.same_name_unverified,
    );
    if !report.ambiguous.is_empty() {
        let _ = writeln!(out, "\nAmbiguous matches ({}):", report.ambiguous.len());
        for g in report.ambiguous.iter().take(per_section) {
            let _ = writeln!(out, "  group: {} in A, {} in B", g.a.len(), g.b.len());
            for l in &g.links {
                let _ = writeln!(
                    out,
                    "    {} <-> {} [{:?} by {}]",
                    l.a, l.b, l.matched_by.confidence, l.matched_by.basis
                );
            }
        }
    }
    for (title, items) in [
        ("Only in A", &report.only_in_a),
        ("Only in B", &report.only_in_b),
    ] {
        if items.is_empty() {
            continue;
        }
        let _ = writeln!(out, "\n{title} ({}):", items.len());
        for e in items.iter().take(per_section) {
            let _ = writeln!(
                out,
                "  {} [{:?}, {:?}]",
                label(&e.record),
                e.absence,
                e.record.strength
            );
        }
    }
    if !report.truncated_sections.is_empty() {
        let _ = writeln!(
            out,
            "\nListing truncated in: {}",
            report.truncated_sections.join(", ")
        );
    }
    out
}

#[cfg(test)]
mod tests;
