//! Version-bound, read-only joins between extracted Arcade sets and MAME XML.
//!
//! The directory name is only the candidate key.  Machine metadata from the
//! selected MAME DAT is the identity authority; member names are used only for
//! the separate storage comparison.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
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
    pub name: String,
    pub kind: MemberEvidenceKind,
    pub checksum: Option<String>,
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
        .map(|r| member_evidence(r, &present, game, by_name, dirs))
        .collect::<Vec<_>>();
    for extra in present
        .iter()
        .filter(|n| !expected_names.contains(n.as_str()))
    {
        members.push(ArcadeMemberEvidence {
            name: extra.clone(),
            kind: MemberEvidenceKind::Extra,
            checksum: None,
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
    game: &DatGameEntry,
    by_name: &BTreeMap<&str, &DatGameEntry>,
    dirs: &BTreeMap<&str, &ArcadeSetDirectory>,
) -> ArcadeMemberEvidence {
    let checksum = rom.sha1.clone().or_else(|| rom.crc32.clone());
    if present.contains(&rom.name) {
        return ArcadeMemberEvidence {
            name: rom.name.clone(),
            kind: MemberEvidenceKind::Present,
            checksum,
        };
    }
    if let Some(merge) = rom.merge.as_deref()
        && provider_has_member(game, merge, by_name, dirs)
    {
        return ArcadeMemberEvidence {
            name: rom.name.clone(),
            kind: MemberEvidenceKind::MergedFromParent,
            checksum,
        };
    }
    if let Some(source) = game.rom_of.as_deref().or(game.clone_of.as_deref())
        && provider_has_member_name(source, &rom.name, dirs)
    {
        return ArcadeMemberEvidence {
            name: rom.name.clone(),
            kind: MemberEvidenceKind::MergedFromParent,
            checksum,
        };
    }
    ArcadeMemberEvidence {
        name: rom.name.clone(),
        kind: MemberEvidenceKind::Missing,
        checksum,
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
