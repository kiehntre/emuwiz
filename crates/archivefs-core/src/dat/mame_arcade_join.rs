//! Version-bound, read-only joins between extracted Arcade sets and MAME XML.
//!
//! The directory name is only the candidate key.  Machine metadata from the
//! selected MAME DAT is the identity authority; member names are used only for
//! the separate storage comparison.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::dat::dependency::SetDependencyReport;
use crate::dat::limits::DatLimits;
use crate::dat::model::{DatGameEntry, DatRomEntry, ParsedDat};
use crate::dat::parsers::mame_listxml::parse_mame_listxml;
use crate::dat::set::{SetIdentity, SetResolution, SetState};
use crate::game_identity::{
    GameIdentityReport, IdentityConfidence, IdentityEvidence, IdentityImageFormat, IdentityKind,
    IdentityPlatform, IdentityProvenance, IdentityStatus,
};
use crate::ingestion::arcade::{ArcadeSetDirectory, discover_extracted_sets};

pub const MAME_0174_SHA256: &str =
    "df9938254e6299a9dc0499ac4d30ef562730d5e1e1d0f8f887402948980fae27";
pub const MAME_0174_VERSION: &str = "0.174";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ArcadeJoinClass {
    ExactSetMatch,
    CloneSet,
    ParentSet,
    BiosSet,
    DeviceSet,
    Ambiguous,
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum MemberEvidenceKind {
    Present,
    Missing,
    Extra,
    MergedFromParent,
    ProvidedByBios,
    ProvidedByDevice,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArcadeMemberEvidence {
    /// DAT target member name.
    pub name: String,
    pub kind: MemberEvidenceKind,
    /// Exact current member name in the containing set/archive.  This is
    /// populated only after the bytes have matched the DAT identity.
    #[serde(default)]
    pub current_name: Option<String>,
    pub checksum: Option<String>,
    /// Checksum observed from the current physical member.  It is retained
    /// separately from `checksum` because the latter is the DAT target.
    #[serde(default)]
    pub observed_sha1: Option<String>,
    #[serde(default)]
    pub observed_crc32: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArcadeDependencyEdge {
    pub kind: String,
    pub target: String,
    pub present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArcadeJoinEvidence {
    pub logical_set_name: String,
    pub dat_set_name: Option<String>,
    pub class: ArcadeJoinClass,
    pub description: Option<String>,
    pub manufacturer: Option<String>,
    pub year: Option<String>,
    pub clone_of: Option<String>,
    pub rom_of: Option<String>,
    pub parent_description: Option<String>,
    pub runnable: Option<String>,
    pub mechanical: bool,
    pub is_bios: bool,
    pub is_device: bool,
    pub expected_member_count: usize,
    pub members: Vec<ArcadeMemberEvidence>,
    pub dependencies: Vec<ArcadeDependencyEdge>,
    pub launchable_normal_game: bool,
    pub dat_version: String,
    pub dat_sha256: String,
    pub dat_path: String,
    pub audited_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArcadeJoinSummary {
    pub logical_sets_inspected: usize,
    pub exact_matches: usize,
    pub parent_sets: usize,
    pub clone_sets: usize,
    pub bios_sets: usize,
    pub device_sets: usize,
    pub mechanical_non_runnable: usize,
    pub unmatched: usize,
    pub ambiguous: usize,
    pub complete: usize,
    pub incomplete: usize,
    pub sets_with_extras: usize,
    pub dependency_edges: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArcadeJoinReport {
    pub dat_path: PathBuf,
    pub dat_sha256: String,
    pub dat_version: String,
    pub dat_machine_count: usize,
    pub scan_root: PathBuf,
    pub evidence: Vec<ArcadeJoinEvidence>,
    pub summary: ArcadeJoinSummary,
    pub layout_estimate: String,
}

pub const MAME_MEMBER_EVIDENCE_VERSION: &str = "mame-member-evidence-v1";

/// One physical member observation.  A row is actionable only when its
/// checksum matched exactly one DAT member; filenames alone never make it so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MamePhysicalMemberEvidence {
    pub logical_set_name: String,
    pub source_path: PathBuf,
    pub current_name: String,
    pub file_size: u64,
    pub modified_time_ns: i64,
    pub sha1: Option<String>,
    pub crc32: Option<String>,
    pub target_set_name: Option<String>,
    pub target_member_name: Option<String>,
    pub actionable: bool,
    pub failure_reason: Option<String>,
    pub evidence_version: String,
    pub observed_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MameEvidenceRefreshReport {
    pub root: PathBuf,
    pub requested_set: Option<String>,
    pub sets_considered: usize,
    pub sets_published: usize,
    pub members_seen: usize,
    pub members_reused: usize,
    pub members_rehashed: usize,
    pub members_actionable: usize,
    pub members_failed: usize,
    pub cache_rows_published: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MameEvidenceCacheStatus {
    pub dat_source_id: String,
    pub physical_rows: usize,
    pub actionable_rows: usize,
    pub logical_sets: usize,
}

/// Refreshes only the requested set's parent/clone family, or all extracted
/// sets below `root` when no set is supplied.  Each set is hashed and
/// published independently, so an interrupted full refresh resumes from its
/// already-published cache rows.
pub fn refresh_mame_member_evidence(
    database: &mut crate::Database,
    dat: &VerifiedMameDat,
    root: &Path,
    requested_set: Option<&str>,
) -> Result<MameEvidenceRefreshReport, String> {
    let discovered = discover_extracted_sets(root)
        .map_err(|error| format!("discover MAME extracted sets: {error}"))?;
    let selected = requested_set.map(|name| {
        let family_parent = dat
            .parsed
            .games
            .iter()
            .find(|game| game.name == name)
            .and_then(|game| game.clone_of.as_deref())
            .unwrap_or(name);
        dat.parsed
            .games
            .iter()
            .filter(|game| {
                game.name == family_parent || game.clone_of.as_deref() == Some(family_parent)
            })
            .map(|game| game.name.clone())
            .collect::<BTreeSet<_>>()
    });
    let dat_source_id = format!("mame-arcade:{}:{}", dat.version, dat.sha256);
    let mut report = MameEvidenceRefreshReport {
        root: root.to_path_buf(),
        requested_set: requested_set.map(str::to_owned),
        ..Default::default()
    };

    for set in discovered.sets.iter().filter(|set| {
        selected
            .as_ref()
            .is_none_or(|names| names.contains(&set.set_name))
    }) {
        report.sets_considered += 1;
        let Some(game) = dat
            .parsed
            .games
            .iter()
            .find(|game| game.name == set.set_name)
        else {
            continue;
        };
        if dat
            .parsed
            .games
            .iter()
            .filter(|candidate| candidate.name == game.name)
            .count()
            != 1
        {
            continue;
        }
        let observed_at = now_utc_string();
        let mut rows = Vec::with_capacity(set.members.len());
        for path in &set.members {
            report.members_seen += 1;
            let current_name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let metadata = match fs::symlink_metadata(path) {
                Ok(metadata) if metadata.file_type().is_file() => metadata,
                Ok(_) => {
                    report.members_failed += 1;
                    rows.push(failed_member_evidence(
                        &set.set_name,
                        path,
                        &current_name,
                        "not a regular file",
                        &observed_at,
                    ));
                    continue;
                }
                Err(error) => {
                    report.members_failed += 1;
                    rows.push(failed_member_evidence(
                        &set.set_name,
                        path,
                        &current_name,
                        &format!("stat failed: {error}"),
                        &observed_at,
                    ));
                    continue;
                }
            };
            let file_size = metadata.len();
            #[cfg(unix)]
            let modified_time_ns = metadata
                .mtime()
                .saturating_mul(1_000_000_000)
                .saturating_add(metadata.mtime_nsec());
            #[cfg(not(unix))]
            let modified_time_ns = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX))
                .unwrap_or(0);
            if let Some(cached) = database
                .cached_mame_member_evidence(
                    &dat_source_id,
                    path,
                    &current_name,
                    file_size,
                    modified_time_ns,
                )
                .map_err(|error| error.to_string())?
            {
                report.members_reused += 1;
                if cached.actionable {
                    report.members_actionable += 1;
                } else {
                    report.members_failed += 1;
                }
                rows.push(cached);
                continue;
            }
            report.members_rehashed += 1;
            let (sha1, crc32) = match hash_member(path) {
                Ok(hashes) => hashes,
                Err(error) => {
                    report.members_failed += 1;
                    rows.push(failed_member_evidence(
                        &set.set_name,
                        path,
                        &current_name,
                        &format!("read failed: {error}"),
                        &observed_at,
                    ));
                    continue;
                }
            };
            let matches = game
                .roms
                .iter()
                .filter(|rom| !is_optional(rom) && !is_nodump(rom))
                .filter(|rom| {
                    rom.sha1
                        .as_deref()
                        .is_some_and(|expected| expected.eq_ignore_ascii_case(&sha1))
                        || rom
                            .crc32
                            .as_deref()
                            .is_some_and(|expected| expected.eq_ignore_ascii_case(&crc32))
                })
                .collect::<Vec<_>>();
            let (target_member_name, failure_reason) = if matches.len() == 1 {
                (Some(matches[0].name.clone()), None)
            } else if matches.is_empty() {
                (
                    None,
                    Some("checksum does not match this DAT set".to_string()),
                )
            } else {
                (
                    None,
                    Some("checksum is ambiguous within this DAT set".to_string()),
                )
            };
            let actionable = target_member_name.is_some();
            if actionable {
                report.members_actionable += 1;
            } else {
                report.members_failed += 1;
            }
            rows.push(MamePhysicalMemberEvidence {
                logical_set_name: set.set_name.clone(),
                source_path: path.clone(),
                current_name,
                file_size,
                modified_time_ns,
                sha1: Some(sha1),
                crc32: Some(crc32),
                target_set_name: actionable.then(|| game.name.clone()),
                target_member_name,
                actionable,
                failure_reason,
                evidence_version: MAME_MEMBER_EVIDENCE_VERSION.to_string(),
                observed_at: observed_at.clone(),
            });
        }
        report.cache_rows_published += database
            .persist_mame_member_evidence_set(&dat_source_id, &set.set_name, &rows)
            .map_err(|error| error.to_string())?;
        report.sets_published += 1;
    }
    Ok(report)
}

fn failed_member_evidence(
    set_name: &str,
    path: &Path,
    current_name: &str,
    reason: &str,
    observed_at: &str,
) -> MamePhysicalMemberEvidence {
    let metadata = fs::symlink_metadata(path).ok();
    MamePhysicalMemberEvidence {
        logical_set_name: set_name.to_string(),
        source_path: path.to_path_buf(),
        current_name: current_name.to_string(),
        file_size: metadata.as_ref().map_or(0, fs::Metadata::len),
        modified_time_ns: 0,
        sha1: None,
        crc32: None,
        target_set_name: None,
        target_member_name: None,
        actionable: false,
        failure_reason: Some(reason.to_string()),
        evidence_version: MAME_MEMBER_EVIDENCE_VERSION.to_string(),
        observed_at: observed_at.to_string(),
    }
}

fn hash_member(path: &Path) -> std::io::Result<(String, String)> {
    let mut file = fs::File::open(path)?;
    let mut sha1 = sha1::Sha1::new();
    let mut crc32 = crate::identity_source::hashing::Crc32::new();
    let mut buffer = [0_u8; 256 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        sha1.update(&buffer[..read]);
        crc32.update(&buffer[..read]);
    }
    Ok((encode_hex(&sha1.finalize()), crc32.finish_hex()))
}

fn now_utc_string() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    format!("unix:{seconds}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedMameDat {
    pub parsed: ParsedDat,
    pub path: PathBuf,
    pub sha256: String,
    pub version: String,
}

/// Projects one persisted, DAT-backed Arcade join into the existing launch
/// planner shape. This is deliberately a projection of join evidence only:
/// it never identifies a set from a filename or from a raw member.
pub fn launch_resolution_for_join(
    evidence: &ArcadeJoinEvidence,
    archive_path: &Path,
) -> Option<SetResolution> {
    if !evidence.launchable_normal_game
        || evidence.class == ArcadeJoinClass::NotFound
        || evidence.class == ArcadeJoinClass::Ambiguous
    {
        return None;
    }
    let complete = evidence.members.iter().all(|member| {
        !matches!(
            member.kind,
            MemberEvidenceKind::Missing | MemberEvidenceKind::Extra
        )
    }) && evidence
        .dependencies
        .iter()
        .all(|dependency| dependency.present);
    let state = if complete {
        SetState::Complete
    } else {
        SetState::Incomplete
    };
    let members_required = evidence
        .members
        .iter()
        .filter(|member| !matches!(member.kind, MemberEvidenceKind::Extra))
        .map(|member| member.name.clone())
        .collect::<Vec<_>>();
    let members_verified = evidence
        .members
        .iter()
        .filter(|member| {
            matches!(
                member.kind,
                MemberEvidenceKind::Present
                    | MemberEvidenceKind::MergedFromParent
                    | MemberEvidenceKind::ProvidedByBios
                    | MemberEvidenceKind::ProvidedByDevice
            )
        })
        .map(|member| member.name.clone())
        .collect::<Vec<_>>();
    let members_borrowed = evidence
        .members
        .iter()
        .filter(|member| {
            matches!(
                member.kind,
                MemberEvidenceKind::MergedFromParent
                    | MemberEvidenceKind::ProvidedByBios
                    | MemberEvidenceKind::ProvidedByDevice
            )
        })
        .map(|member| member.name.clone())
        .collect::<Vec<_>>();
    Some(SetResolution {
        identity: SetIdentity {
            source_id: format!(
                "mame-arcade:{}:{}",
                evidence.dat_version, evidence.dat_sha256
            ),
            game_name: evidence.dat_set_name.clone()?,
        },
        archive_path: archive_path.to_path_buf(),
        state,
        members_required,
        members_verified,
        members_bad: Vec::new(),
        members_optional: Vec::new(),
        members_borrowed,
        disks_required: Vec::new(),
        disks_verified: Vec::new(),
        disks_parent_required: Vec::new(),
        dependencies: SetDependencyReport {
            state: if evidence.dependencies.is_empty() {
                crate::dat::dependency::DependencyState::NotApplicable
            } else if evidence
                .dependencies
                .iter()
                .all(|dependency| dependency.present)
            {
                crate::dat::dependency::DependencyState::Satisfied
            } else {
                crate::dat::dependency::DependencyState::Missing
            },
            requirements: Vec::new(),
        },
    })
}

/// Converts an authoritative MAME join into the catalogue's trusted identity
/// shape. The logical set is the sole identity target: member paths are
/// deliberately absent from the identity provenance and cannot become game
/// keys. The set shortname only becomes verified after the checksum-pinned
/// DAT supplied a real machine entry with a non-empty member manifest.
pub fn identity_report_for_join(
    evidence: &ArcadeJoinEvidence,
    archive_path: &Path,
) -> Option<GameIdentityReport> {
    if !evidence.launchable_normal_game
        || evidence.dat_version != MAME_0174_VERSION
        || evidence.dat_sha256 != MAME_0174_SHA256
        || matches!(
            evidence.class,
            ArcadeJoinClass::NotFound
                | ArcadeJoinClass::Ambiguous
                | ArcadeJoinClass::BiosSet
                | ArcadeJoinClass::DeviceSet
        )
        || evidence.expected_member_count == 0
        || evidence.members.is_empty()
    {
        return None;
    }
    let machine = evidence.dat_set_name.as_deref()?;
    if machine != evidence.logical_set_name {
        return None;
    }
    let provenance = IdentityProvenance {
        archive_path: archive_path.to_path_buf(),
        member_path: None,
        member_index: None,
        method: format!(
            "checksum-pinned MAME {} DAT logical-set join ({})",
            evidence.dat_version, evidence.dat_sha256
        ),
    };
    Some(GameIdentityReport {
        archive_path: archive_path.to_path_buf(),
        platform: IdentityPlatform::Arcade,
        // This report describes a logical set rather than one byte-image
        // container. The explicit Arcade platform and MAME identity kind
        // carry the trust; no raw member format is manufactured here.
        format: IdentityImageFormat::Unsupported,
        evidence: vec![
            IdentityEvidence {
                kind: IdentityKind::Platform,
                status: IdentityStatus::Verified,
                value: Some("Arcade".into()),
                confidence: IdentityConfidence::CatalogueContext,
                provenance: provenance.clone(),
                diagnostic:
                    "logical Arcade set attached to an exact machine in the verified MAME DAT"
                        .into(),
            },
            IdentityEvidence {
                kind: IdentityKind::MameMachineName,
                status: IdentityStatus::Verified,
                value: Some(machine.to_string()),
                confidence: IdentityConfidence::StructuredMetadata,
                provenance,
                diagnostic: "exact MAME machine shortname from the verified logical-set join"
                    .into(),
            },
        ],
        warnings: Vec::new(),
        bytes_read: 0,
        archive_members_inspected: evidence.members.len(),
        metadata_paths_inspected: 1,
        nested_container_depth: 0,
        complete: true,
    })
}

pub fn load_verified_mame_0174(path: &Path) -> Result<VerifiedMameDat, String> {
    let bytes = fs::read(path).map_err(|e| format!("read MAME DAT: {e}"))?;
    let digest = Sha256::digest(&bytes);
    let sha256 = digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if sha256 != MAME_0174_SHA256 {
        return Err(format!(
            "refusing DAT with SHA-256 {sha256}; expected {MAME_0174_SHA256}"
        ));
    }
    let parsed = parse_mame_listxml(path, DatLimits::default())
        .map_err(|e| format!("parse MAME XML: {e}"))?
        .dat;
    // This DAT is Logiqx-shaped datafile XML with `<machine>` entries rather
    // than current `-listxml`'s `<mame build=...>` root. The existing bounded
    // MAME machine parser supplies the entries; this exact header assertion
    // proves the publisher's internal revision without filename inference.
    let header_proof = String::from_utf8_lossy(&bytes);
    if !header_proof.contains("<description>MAME Arcade 0.174</description>") {
        return Err("refusing DAT without internal MAME 0.174 header description".into());
    }
    Ok(VerifiedMameDat {
        parsed,
        path: path.to_path_buf(),
        sha256,
        version: MAME_0174_VERSION.to_string(),
    })
}

pub fn join_extracted_arcade_root(
    dat: &VerifiedMameDat,
    root: &Path,
    audited_at: &str,
) -> Result<ArcadeJoinReport, String> {
    let discovered =
        discover_extracted_sets(root).map_err(|e| format!("discover Arcade sets: {e}"))?;
    let mut by_name = BTreeMap::new();
    let mut ambiguous_names = BTreeSet::new();
    let mut clone_parents = BTreeSet::new();
    for game in &dat.parsed.games {
        if by_name.insert(game.name.as_str(), game).is_some() {
            ambiguous_names.insert(game.name.as_str());
        }
        if let Some(parent) = game.clone_of.as_deref() {
            clone_parents.insert(parent);
        }
    }
    let mut dirs = BTreeMap::new();
    for set in &discovered.sets {
        dirs.insert(set.set_name.as_str(), set);
    }
    let mut evidence = Vec::with_capacity(discovered.sets.len());
    for set in &discovered.sets {
        evidence.push(join_one_indexed(
            dat,
            set,
            &by_name,
            &ambiguous_names,
            &clone_parents,
            &dirs,
            audited_at,
        ));
    }
    let mut summary = ArcadeJoinSummary {
        logical_sets_inspected: evidence.len(),
        ..Default::default()
    };
    for item in &evidence {
        match item.class {
            ArcadeJoinClass::ExactSetMatch => summary.exact_matches += 1,
            ArcadeJoinClass::ParentSet => summary.parent_sets += 1,
            ArcadeJoinClass::CloneSet => summary.clone_sets += 1,
            ArcadeJoinClass::BiosSet => summary.bios_sets += 1,
            ArcadeJoinClass::DeviceSet => summary.device_sets += 1,
            ArcadeJoinClass::NotFound => summary.unmatched += 1,
            ArcadeJoinClass::Ambiguous => summary.ambiguous += 1,
        }
        if item.mechanical || item.runnable.as_deref() == Some("no") {
            summary.mechanical_non_runnable += 1;
        }
        if item.class != ArcadeJoinClass::NotFound && item.class != ArcadeJoinClass::Ambiguous {
            if item
                .members
                .iter()
                .any(|m| m.kind == MemberEvidenceKind::Missing)
            {
                summary.incomplete += 1;
            } else {
                summary.complete += 1;
            }
            if item
                .members
                .iter()
                .any(|m| m.kind == MemberEvidenceKind::Extra)
            {
                summary.sets_with_extras += 1;
            }
        }
        summary.dependency_edges += item.dependencies.len();
    }
    let layout_estimate = estimate_layout(&evidence);
    Ok(ArcadeJoinReport {
        dat_path: dat.path.clone(),
        dat_sha256: dat.sha256.clone(),
        dat_version: dat.version.clone(),
        dat_machine_count: dat.parsed.games.len(),
        scan_root: root.to_path_buf(),
        evidence,
        summary,
        layout_estimate,
    })
}

#[cfg(test)]
fn join_one(
    dat: &VerifiedMameDat,
    set: &ArcadeSetDirectory,
    by_name: &BTreeMap<&str, &DatGameEntry>,
    dirs: &BTreeMap<&str, &ArcadeSetDirectory>,
    audited_at: &str,
) -> ArcadeJoinEvidence {
    let mut seen_names = BTreeSet::new();
    let mut ambiguous_names = BTreeSet::new();
    for game in &dat.parsed.games {
        if !seen_names.insert(game.name.as_str()) {
            ambiguous_names.insert(game.name.as_str());
        }
    }
    let clone_parents = dat
        .parsed
        .games
        .iter()
        .filter_map(|game| game.clone_of.as_deref())
        .collect::<BTreeSet<_>>();
    join_one_indexed(
        dat,
        set,
        by_name,
        &ambiguous_names,
        &clone_parents,
        dirs,
        audited_at,
    )
}

fn join_one_indexed(
    dat: &VerifiedMameDat,
    set: &ArcadeSetDirectory,
    by_name: &BTreeMap<&str, &DatGameEntry>,
    ambiguous_names: &BTreeSet<&str>,
    clone_parents: &BTreeSet<&str>,
    dirs: &BTreeMap<&str, &ArcadeSetDirectory>,
    audited_at: &str,
) -> ArcadeJoinEvidence {
    let (game, class) = if ambiguous_names.contains(set.set_name.as_str()) {
        (None, ArcadeJoinClass::Ambiguous)
    } else if let Some(g) = by_name.get(set.set_name.as_str()).copied() {
        let class = if flag(&g.is_bios) {
            ArcadeJoinClass::BiosSet
        } else if flag(&g.is_device) {
            ArcadeJoinClass::DeviceSet
        } else if g.clone_of.is_some() {
            ArcadeJoinClass::CloneSet
        } else if clone_parents.contains(g.name.as_str()) {
            ArcadeJoinClass::ParentSet
        } else {
            ArcadeJoinClass::ExactSetMatch
        };
        (Some(g), class)
    } else {
        (None, ArcadeJoinClass::NotFound)
    };
    let Some(game) = game else {
        return ArcadeJoinEvidence {
            logical_set_name: set.set_name.clone(),
            dat_set_name: None,
            class,
            description: None,
            manufacturer: None,
            year: None,
            clone_of: None,
            rom_of: None,
            parent_description: None,
            runnable: None,
            mechanical: false,
            is_bios: false,
            is_device: false,
            expected_member_count: 0,
            members: Vec::new(),
            dependencies: Vec::new(),
            launchable_normal_game: false,
            dat_version: dat.version.clone(),
            dat_sha256: dat.sha256.clone(),
            dat_path: dat.path.to_string_lossy().into_owned(),
            audited_at: audited_at.to_string(),
        };
    };
    let present = set
        .members
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect::<BTreeSet<_>>();
    let physical = physical_member_evidence(&set.members);
    let expected = game
        .roms
        .iter()
        .filter(|r| !is_optional(r) && !is_nodump(r))
        .collect::<Vec<_>>();
    let expected_names = expected
        .iter()
        .map(|r| r.name.as_str())
        .collect::<BTreeSet<_>>();
    let mut members = expected
        .iter()
        .map(|r| member_evidence(r, &present, &physical, game, by_name, dirs))
        .collect::<Vec<_>>();
    for extra in present
        .iter()
        .filter(|n| !expected_names.contains(n.as_str()))
    {
        members.push(ArcadeMemberEvidence {
            name: extra.clone(),
            kind: MemberEvidenceKind::Extra,
            current_name: Some(extra.clone()),
            checksum: None,
            observed_sha1: physical
                .iter()
                .find(|member| member.name == *extra)
                .map(|member| member.sha1.clone()),
            observed_crc32: physical
                .iter()
                .find(|member| member.name == *extra)
                .map(|member| member.crc32.clone()),
        });
    }
    let mut dependencies = Vec::new();
    if let Some(parent) = game.clone_of.as_deref() {
        dependencies.push(ArcadeDependencyEdge {
            kind: "ParentSet".into(),
            target: parent.into(),
            present: by_name.contains_key(parent) && dirs.contains_key(parent),
        });
    }
    if let Some(source) = game.rom_of.as_deref() {
        dependencies.push(ArcadeDependencyEdge {
            kind: "RomSource".into(),
            target: source.into(),
            present: dirs.contains_key(source),
        });
    }
    for device in &game.device_refs {
        if let Some(name) = device.name.as_deref() {
            dependencies.push(ArcadeDependencyEdge {
                kind: "Device".into(),
                target: name.into(),
                present: device_dependency_present(name, by_name, dirs),
            });
        }
    }
    for rom in &game.roms {
        if let Some(bios) = rom.bios.as_deref() {
            dependencies.push(ArcadeDependencyEdge {
                kind: "Bios".into(),
                target: bios.into(),
                present: game
                    .bios_sets
                    .iter()
                    .any(|b| b.name.as_deref() == Some(bios)),
            });
        }
    }
    let mechanical = game
        .original_metadata
        .fields
        .get("ismechanical")
        .is_some_and(|v| v == "yes");
    let normal = !flag(&game.is_bios)
        && !flag(&game.is_device)
        && !mechanical
        && game.runnable.as_deref() != Some("no");
    let parent_description = game
        .clone_of
        .as_deref()
        .and_then(|p| by_name.get(p).and_then(|g| g.description.clone()));
    ArcadeJoinEvidence {
        logical_set_name: set.set_name.clone(),
        dat_set_name: Some(game.name.clone()),
        class,
        description: game.description.clone(),
        manufacturer: game.manufacturer.clone(),
        year: game.year.clone(),
        clone_of: game.clone_of.clone(),
        rom_of: game.rom_of.clone(),
        parent_description,
        runnable: game.runnable.clone(),
        mechanical,
        is_bios: flag(&game.is_bios),
        is_device: flag(&game.is_device),
        expected_member_count: expected.len(),
        members,
        dependencies,
        launchable_normal_game: normal,
        dat_version: dat.version.clone(),
        dat_sha256: dat.sha256.clone(),
        dat_path: dat.path.to_string_lossy().into_owned(),
        audited_at: audited_at.to_string(),
    }
}

/// A MAME `<device_ref>` always requires a matching device definition, but
/// only a device that declares required ROM/disk payload needs a same-named
/// storage set. CPU/timer/etc. definitions such as `i486` are compiled into
/// MAME and intentionally have no ROM directory of their own.
fn device_dependency_present(
    name: &str,
    by_name: &BTreeMap<&str, &DatGameEntry>,
    dirs: &BTreeMap<&str, &ArcadeSetDirectory>,
) -> bool {
    let Some(device) = by_name
        .get(name)
        .copied()
        .filter(|game| flag(&game.is_device))
    else {
        return false;
    };
    let requires_storage = device
        .roms
        .iter()
        .any(|rom| !is_optional(rom) && !is_nodump(rom))
        || device.disks.iter().any(|disk| {
            !disk
                .optional
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case("yes"))
                && !disk
                    .status
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case("nodump"))
        });
    !requires_storage || dirs.contains_key(name)
}

fn member_evidence(
    rom: &DatRomEntry,
    present: &BTreeSet<String>,
    physical: &[PhysicalMemberEvidence],
    game: &DatGameEntry,
    by_name: &BTreeMap<&str, &DatGameEntry>,
    dirs: &BTreeMap<&str, &ArcadeSetDirectory>,
) -> ArcadeMemberEvidence {
    let checksum = rom.sha1.clone().or_else(|| rom.crc32.clone());
    if let Some(location) = exact_member_location(rom, physical) {
        return ArcadeMemberEvidence {
            name: rom.name.clone(),
            kind: MemberEvidenceKind::Present,
            current_name: Some(location.name.clone()),
            checksum,
            observed_sha1: Some(location.sha1.clone()),
            observed_crc32: Some(location.crc32.clone()),
        };
    }
    // Keep the old completeness semantics for a filename match whose bytes
    // cannot be proved.  The absence of `current_name` makes it explicitly
    // non-actionable to the normaliser; filenames never become identity.
    if present.contains(&rom.name) {
        return ArcadeMemberEvidence {
            name: rom.name.clone(),
            kind: MemberEvidenceKind::Present,
            current_name: None,
            checksum,
            observed_sha1: None,
            observed_crc32: None,
        };
    }
    if let Some(merge) = rom.merge.as_deref()
        && provider_has_member(game, merge, by_name, dirs)
    {
        return ArcadeMemberEvidence {
            name: rom.name.clone(),
            kind: MemberEvidenceKind::MergedFromParent,
            current_name: None,
            checksum,
            observed_sha1: None,
            observed_crc32: None,
        };
    }
    if let Some(source) = game.rom_of.as_deref().or(game.clone_of.as_deref())
        && provider_has_member_name(source, &rom.name, dirs)
    {
        return ArcadeMemberEvidence {
            name: rom.name.clone(),
            kind: MemberEvidenceKind::MergedFromParent,
            current_name: None,
            checksum,
            observed_sha1: None,
            observed_crc32: None,
        };
    }
    ArcadeMemberEvidence {
        name: rom.name.clone(),
        kind: MemberEvidenceKind::Missing,
        current_name: None,
        checksum,
        observed_sha1: None,
        observed_crc32: None,
    }
}

#[derive(Debug, Clone)]
struct PhysicalMemberEvidence {
    name: String,
    sha1: String,
    crc32: String,
}

fn physical_member_evidence(paths: &[PathBuf]) -> Vec<PhysicalMemberEvidence> {
    paths
        .iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_string_lossy().into_owned();
            let mut file = fs::File::open(path).ok()?;
            let mut sha1 = sha1::Sha1::new();
            let mut crc32 = crate::identity_source::hashing::Crc32::new();
            let mut buffer = [0_u8; 256 * 1024];
            loop {
                let read = file.read(&mut buffer).ok()?;
                if read == 0 {
                    break;
                }
                sha1.update(&buffer[..read]);
                crc32.update(&buffer[..read]);
            }
            Some(PhysicalMemberEvidence {
                name,
                sha1: encode_hex(&sha1.finalize()),
                crc32: crc32.finish_hex(),
            })
        })
        .collect()
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn exact_member_location<'a>(
    rom: &DatRomEntry,
    physical: &'a [PhysicalMemberEvidence],
) -> Option<&'a PhysicalMemberEvidence> {
    let matches = physical
        .iter()
        .filter(|member| {
            (rom.sha1.is_some() || rom.crc32.is_some())
                && rom
                    .sha1
                    .as_deref()
                    .is_none_or(|expected| expected.eq_ignore_ascii_case(&member.sha1))
                && rom
                    .crc32
                    .as_deref()
                    .is_none_or(|expected| expected.eq_ignore_ascii_case(&member.crc32))
        })
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        Some(matches[0])
    } else {
        None
    }
}

fn provider_has_member(
    game: &DatGameEntry,
    member: &str,
    by_name: &BTreeMap<&str, &DatGameEntry>,
    dirs: &BTreeMap<&str, &ArcadeSetDirectory>,
) -> bool {
    game.rom_of
        .as_deref()
        .or(game.clone_of.as_deref())
        .and_then(|p| by_name.get(p).map(|_| p))
        .is_some_and(|p| provider_has_member_name(p, member, dirs))
}
fn provider_has_member_name(
    provider: &str,
    member: &str,
    dirs: &BTreeMap<&str, &ArcadeSetDirectory>,
) -> bool {
    dirs.get(provider).is_some_and(|s| {
        s.members
            .iter()
            .any(|p| p.file_name().is_some_and(|n| n.to_string_lossy() == member))
    })
}
fn is_optional(r: &DatRomEntry) -> bool {
    r.optional
        .as_deref()
        .is_some_and(|v| v.eq_ignore_ascii_case("yes"))
}
fn is_nodump(r: &DatRomEntry) -> bool {
    r.status
        .as_deref()
        .is_some_and(|v| v.eq_ignore_ascii_case("nodump"))
}
fn flag(v: &Option<String>) -> bool {
    v.as_deref().is_some_and(|v| v.eq_ignore_ascii_case("yes"))
}
fn estimate_layout(items: &[ArcadeJoinEvidence]) -> String {
    let merged = items
        .iter()
        .filter(|i| {
            i.members
                .iter()
                .any(|m| m.kind == MemberEvidenceKind::MergedFromParent)
        })
        .count();
    let extras = items
        .iter()
        .filter(|i| {
            i.members
                .iter()
                .any(|m| m.kind == MemberEvidenceKind::Extra)
        })
        .count();
    if merged > items.len() / 3 && extras < items.len() / 3 {
        "mostly merged/extracted".into()
    } else if merged == 0 {
        "mostly non-merged or split/extracted".into()
    } else {
        "mixed extracted".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn game(name: &str) -> DatGameEntry {
        DatGameEntry {
            name: name.into(),
            runnable: Some("yes".into()),
            ..Default::default()
        }
    }

    fn dat(games: Vec<DatGameEntry>) -> VerifiedMameDat {
        let source = crate::dat::model::DatSource {
            format: crate::dat::model::DatFormat::Logiqx,
            ecosystem: crate::dat::model::DatEcosystem::MAMEArcade,
            file_path: "mame.dat".into(),
            name: Some("MAME".into()),
            description: Some("MAME Arcade 0.174".into()),
            version: Some("-not specified-".into()),
            author: None,
            homepage: None,
            clrmamepro_header: None,
            entry_count: games.len(),
            rom_count: 0,
            parse_warnings: Vec::new(),
            packing_policy: crate::dat::model::DatPackingPolicy::Standard,
        };
        VerifiedMameDat {
            parsed: ParsedDat { source, games },
            path: PathBuf::from("mame.dat"),
            sha256: MAME_0174_SHA256.into(),
            version: MAME_0174_VERSION.into(),
        }
    }

    #[test]
    fn member_evidence_cache_reuses_exact_fingerprint_and_rejects_stale_input() {
        let directory = tempfile::tempdir().unwrap();
        let member_path = directory.path().join("wrong-name.bin");
        fs::write(&member_path, b"cache fixture").unwrap();
        let metadata = fs::metadata(&member_path).unwrap();
        #[cfg(unix)]
        let modified_time_ns = metadata
            .mtime()
            .saturating_mul(1_000_000_000)
            .saturating_add(metadata.mtime_nsec());
        #[cfg(not(unix))]
        let modified_time_ns = 0;
        let row = MamePhysicalMemberEvidence {
            logical_set_name: "fixture".into(),
            source_path: member_path.clone(),
            current_name: "wrong-name.bin".into(),
            file_size: metadata.len(),
            modified_time_ns,
            sha1: Some("a".repeat(40)),
            crc32: Some("b".repeat(8)),
            target_set_name: Some("fixture".into()),
            target_member_name: Some("correct.bin".into()),
            actionable: true,
            failure_reason: None,
            evidence_version: MAME_MEMBER_EVIDENCE_VERSION.into(),
            observed_at: "test".into(),
        };
        let mut database =
            crate::Database::open_or_create(directory.path().join("library.sqlite3")).unwrap();
        database
            .persist_mame_member_evidence_set("mame-arcade:test", "fixture", &[row])
            .unwrap();
        assert!(
            database
                .cached_mame_member_evidence(
                    "mame-arcade:test",
                    &member_path,
                    "wrong-name.bin",
                    metadata.len(),
                    modified_time_ns,
                )
                .unwrap()
                .is_some()
        );
        assert!(
            database
                .cached_mame_member_evidence(
                    "mame-arcade:test",
                    &member_path,
                    "wrong-name.bin",
                    metadata.len() + 1,
                    modified_time_ns,
                )
                .unwrap()
                .is_none()
        );
        database
            .persist_mame_member_evidence_set("mame-arcade:test", "fixture", &[])
            .unwrap();
        assert!(
            database
                .cached_mame_member_evidence(
                    "mame-arcade:test",
                    &member_path,
                    "wrong-name.bin",
                    metadata.len(),
                    modified_time_ns,
                )
                .unwrap()
                .is_none()
        );
    }

    fn set(root: &Path, name: &str, members: &[&str]) -> ArcadeSetDirectory {
        let path = root.join(name);
        fs::create_dir_all(&path).unwrap();
        let mut paths = Vec::new();
        for member in members {
            let path = path.join(member);
            fs::write(&path, b"synthetic legal fixture").unwrap();
            paths.push(path);
        }
        ArcadeSetDirectory {
            path,
            set_name: name.into(),
            members: paths,
        }
    }

    #[test]
    fn wrong_version_is_refused_before_join() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wrong.dat");
        fs::write(
            &path,
            b"<datafile><header><description>MAME Arcade 0.173</description></header></datafile>",
        )
        .unwrap();
        let err = load_verified_mame_0174(&path);
        assert!(err.is_err());
    }

    #[test]
    fn flags_are_strict() {
        assert!(flag(&Some("yes".into())));
        assert!(!flag(&Some("no".into())));
    }

    #[test]
    fn exact_parent_clone_romof_and_merged_members_are_preserved() {
        let root = tempfile::tempdir().unwrap();
        let mut parent = game("parent");
        parent.roms.push(DatRomEntry {
            name: "shared.bin".into(),
            sha1: Some("a".repeat(40)),
            ..Default::default()
        });
        let mut clone = game("clone");
        clone.clone_of = Some("parent".into());
        clone.rom_of = Some("parent".into());
        clone.roms.push(DatRomEntry {
            name: "shared.bin".into(),
            merge: Some("shared.bin".into()),
            ..Default::default()
        });
        clone.roms.push(DatRomEntry {
            name: "clone.bin".into(),
            ..Default::default()
        });
        let verified = dat(vec![parent, clone]);
        let parent_set = set(root.path(), "parent", &["shared.bin"]);
        let clone_set = set(root.path(), "clone", &["extra.bin"]);
        let mut by_name = BTreeMap::new();
        by_name.insert("parent", &verified.parsed.games[0]);
        by_name.insert("clone", &verified.parsed.games[1]);
        let mut dirs = BTreeMap::new();
        dirs.insert("parent", &parent_set);
        dirs.insert("clone", &clone_set);
        let evidence = join_one(&verified, &clone_set, &by_name, &dirs, "test");
        assert_eq!(evidence.class, ArcadeJoinClass::CloneSet);
        assert_eq!(
            evidence
                .members
                .iter()
                .find(|m| m.name == "shared.bin")
                .unwrap()
                .kind,
            MemberEvidenceKind::MergedFromParent
        );
        assert!(
            evidence
                .members
                .iter()
                .any(|m| m.name == "clone.bin" && m.kind == MemberEvidenceKind::Missing)
        );
        assert!(
            evidence
                .members
                .iter()
                .any(|m| m.name == "extra.bin" && m.kind == MemberEvidenceKind::Extra)
        );
    }

    #[test]
    fn exact_member_location_is_persisted_only_for_unique_checksum_match() {
        let root = tempfile::tempdir().unwrap();
        let bytes = b"verified rom bytes";
        let mut digest = sha1::Sha1::new();
        digest.update(bytes);
        let sha1 = encode_hex(&digest.finalize());
        let mut machine = game("machine");
        machine.roms.push(DatRomEntry {
            name: "dat-name.bin".into(),
            sha1: Some(sha1.clone()),
            ..Default::default()
        });
        let verified = dat(vec![machine]);
        let set = set(root.path(), "machine", &["wrong-name.bin"]);
        fs::write(root.path().join("machine").join("wrong-name.bin"), bytes).unwrap();
        let mut by_name = BTreeMap::new();
        by_name.insert("machine", &verified.parsed.games[0]);
        let mut dirs = BTreeMap::new();
        dirs.insert("machine", &set);

        let evidence = join_one(&verified, &set, &by_name, &dirs, "test");
        let member = evidence
            .members
            .iter()
            .find(|member| member.name == "dat-name.bin")
            .unwrap();
        assert_eq!(member.current_name.as_deref(), Some("wrong-name.bin"));
        assert_eq!(member.observed_sha1.as_deref(), Some(sha1.as_str()));

        fs::write(root.path().join("machine").join("duplicate.bin"), bytes).unwrap();
        let duplicate_set = ArcadeSetDirectory {
            path: set.path.clone(),
            set_name: set.set_name.clone(),
            members: vec![
                root.path().join("machine").join("wrong-name.bin"),
                root.path().join("machine").join("duplicate.bin"),
            ],
        };
        dirs.insert("machine", &duplicate_set);
        let evidence = join_one(&verified, &duplicate_set, &by_name, &dirs, "test");
        let member = evidence
            .members
            .iter()
            .find(|member| member.name == "dat-name.bin")
            .unwrap();
        assert!(member.current_name.is_none());
        assert!(member.observed_sha1.is_none());
    }

    #[test]
    fn romless_device_definition_is_satisfied_without_a_fake_device_set() {
        let root = tempfile::tempdir().unwrap();
        let mut parent = game("parent");
        parent.roms.push(DatRomEntry {
            name: "parent.bin".into(),
            ..Default::default()
        });
        let mut clone = game("clone");
        clone.clone_of = Some("parent".into());
        clone.rom_of = Some("parent".into());
        clone
            .device_refs
            .push(crate::dat::model::DatDeviceRefEntry {
                name: Some("i486".into()),
            });
        clone.roms.push(DatRomEntry {
            name: "clone.bin".into(),
            ..Default::default()
        });
        let mut cpu = game("i486");
        cpu.is_device = Some("yes".into());
        cpu.runnable = Some("no".into());
        let verified = dat(vec![parent, clone, cpu]);
        let parent_set = set(root.path(), "parent", &["parent.bin"]);
        let clone_set = set(root.path(), "clone", &["clone.bin"]);
        let mut by_name = BTreeMap::new();
        for game in &verified.parsed.games {
            by_name.insert(game.name.as_str(), game);
        }
        let mut dirs = BTreeMap::new();
        dirs.insert("parent", &parent_set);
        dirs.insert("clone", &clone_set);

        let evidence = join_one(&verified, &clone_set, &by_name, &dirs, "test");
        assert_eq!(evidence.clone_of.as_deref(), Some("parent"));
        assert!(evidence.dependencies.iter().any(|dependency| {
            dependency.kind == "Device" && dependency.target == "i486" && dependency.present
        }));
        assert_eq!(
            launch_resolution_for_join(&evidence, &clone_set.path)
                .unwrap()
                .state,
            SetState::Complete
        );
    }

    #[test]
    fn device_with_required_payload_stays_missing_without_its_set() {
        let root = tempfile::tempdir().unwrap();
        let mut machine = game("machine");
        machine
            .device_refs
            .push(crate::dat::model::DatDeviceRefEntry {
                name: Some("soundboard".into()),
            });
        machine.roms.push(DatRomEntry {
            name: "game.bin".into(),
            ..Default::default()
        });
        let mut device = game("soundboard");
        device.is_device = Some("yes".into());
        device.runnable = Some("no".into());
        device.roms.push(DatRomEntry {
            name: "device.bin".into(),
            ..Default::default()
        });
        let verified = dat(vec![machine, device]);
        let machine_set = set(root.path(), "machine", &["game.bin"]);
        let mut by_name = BTreeMap::new();
        for game in &verified.parsed.games {
            by_name.insert(game.name.as_str(), game);
        }
        let mut dirs = BTreeMap::new();
        dirs.insert("machine", &machine_set);

        let evidence = join_one(&verified, &machine_set, &by_name, &dirs, "test");
        assert!(evidence.dependencies.iter().any(|dependency| {
            dependency.kind == "Device" && dependency.target == "soundboard" && !dependency.present
        }));
        assert_eq!(
            launch_resolution_for_join(&evidence, &machine_set.path)
                .unwrap()
                .state,
            SetState::Incomplete
        );
    }

    #[test]
    fn bios_device_mechanical_unmatched_and_ambiguous_are_classified() {
        let root = tempfile::tempdir().unwrap();
        let mut bios = game("bios");
        bios.is_bios = Some("yes".into());
        bios.runnable = Some("no".into());
        let mut device = game("device");
        device.is_device = Some("yes".into());
        let mut mechanical = game("mech");
        mechanical
            .original_metadata
            .fields
            .insert("ismechanical".into(), "yes".into());
        let games = vec![bios, device, mechanical, game("dup"), game("dup")];
        let verified = dat(games);
        let mut by_name = BTreeMap::new();
        for game in &verified.parsed.games {
            by_name.entry(game.name.as_str()).or_insert(game);
        }
        let bios_set = set(root.path(), "bios", &["bios.bin", "x.bin"]);
        let device_set = set(root.path(), "device", &["device.bin", "x.bin"]);
        let mech_set = set(root.path(), "mech", &["mech.bin", "x.bin"]);
        let dup_set = set(root.path(), "dup", &["x.bin", "y.bin"]);
        let mut dirs = BTreeMap::new();
        dirs.insert("bios", &bios_set);
        dirs.insert("device", &device_set);
        dirs.insert("mech", &mech_set);
        dirs.insert("dup", &dup_set);
        assert_eq!(
            join_one(&verified, &bios_set, &by_name, &dirs, "test").class,
            ArcadeJoinClass::BiosSet
        );
        assert_eq!(
            join_one(&verified, &device_set, &by_name, &dirs, "test").class,
            ArcadeJoinClass::DeviceSet
        );
        assert!(join_one(&verified, &mech_set, &by_name, &dirs, "test").mechanical);
        assert_eq!(
            join_one(&verified, &dup_set, &by_name, &dirs, "test").class,
            ArcadeJoinClass::Ambiguous
        );
        let missing = set(root.path(), "missing", &["x.bin", "y.bin"]);
        assert_eq!(
            join_one(&verified, &missing, &by_name, &dirs, "test").class,
            ArcadeJoinClass::NotFound
        );
    }
}
