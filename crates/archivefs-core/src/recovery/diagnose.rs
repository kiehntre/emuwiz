//! Diagnosis: turn a game that cannot start into findings and a plan.
//!
//! Read-only. The costliest step reads a ZIP's central directory, one small
//! cue sheet and two firmware files; nothing here is fit for a UI thread, and
//! callers run it on a worker.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde::Serialize;

use super::capability::{AMIGA_CD32, CapabilityStatus, capability_rows};
use super::disc_zip::{DiscSetInspection, DiscSetLimits, DiscSetRefusal, inspect_disc_zip};
use super::model::{
    ActionEffect, ActionKind, ActionPerformer, Blocking, Confidence, Evidence, EvidenceSource,
    Finding, FindingCode, ProposedAction, RecoveryReport, RecoverySubject, RecoveryVerdict,
};
use super::prepared_media::{PreparedMediaStore, SetState};
use crate::safe_read::TrustedRoots;

/// The RetroArch core that runs CD32 discs.
pub const CD32_CORE_STEM: &str = "puae";

/// A firmware file the CD32 route needs, by the name the core looks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownFirmware {
    pub file_name: &'static str,
    pub description: &'static str,
    /// SHA-1 of the known release. A checksum is a fact about the file the
    /// person already owns; the engine never obtains firmware.
    pub sha1: &'static str,
    pub size_bytes: u64,
}

pub const CD32_FIRMWARE: &[KnownFirmware] = &[
    KnownFirmware {
        file_name: "kick40060.CD32",
        description: "CD32 Kickstart 3.1 (40.60)",
        sha1: "3525be8887f79b5929e017b42380a79edfee542d",
        size_bytes: 524_288,
    },
    KnownFirmware {
        file_name: "kick40060.CD32.ext",
        description: "CD32 extended ROM (40.60)",
        sha1: "5bef3d628ce59cc02a66e6e4ae0da48f60e78f7f",
        size_bytes: 524_288,
    },
];

/// One RetroArch installation EmuWiz can start, as far as this engine needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RetroArchHost {
    pub label: String,
    pub system_directory: Option<PathBuf>,
    pub core_stems: Vec<String>,
}

/// What is installed on this machine. Gathered by the caller (it already has
/// the RetroArch discovery report) so diagnosis itself stays cheap and testable.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct HostFacts {
    /// Installations EmuWiz's RetroArch launcher accepts (native, AppImage).
    pub retroarch: Vec<RetroArchHost>,
    /// False when discovery itself failed; absence of a core then proves nothing.
    pub retroarch_discovered: bool,
}

impl HostFacts {
    pub fn from_retroarch_report(
        report: &crate::emulator_environment::retroarch::RetroArchEnvironmentReport,
    ) -> Self {
        use crate::emulator_environment::retroarch::{PathPurpose, ProfileKind};
        let retroarch = report
            .profiles
            .iter()
            .filter(|profile| {
                matches!(
                    profile.profile_kind,
                    ProfileKind::Native | ProfileKind::AppImage
                )
            })
            .map(|profile| RetroArchHost {
                label: format!("{:?} RetroArch", profile.profile_kind),
                system_directory: profile
                    .paths
                    .iter()
                    .find(|path| path.purpose == PathPurpose::System)
                    .and_then(|path| path.resolved_path.as_ref())
                    .filter(|path| !path.lossy)
                    .map(|path| PathBuf::from(&path.display)),
                core_stems: profile
                    .cores
                    .iter()
                    .map(|core| core.core_stem.clone())
                    .collect(),
            })
            .collect();
        Self {
            retroarch,
            retroarch_discovered: true,
        }
    }

    /// Discovers RetroArch on this machine with the same read-only probe the
    /// launcher uses.
    pub fn probe() -> Self {
        use crate::emulator_environment::retroarch::{
            DiscoveryEnvironment, discover_retroarch_environment,
        };
        match discover_retroarch_environment(
            &crate::emulator_environment::HostReadOnlyFilesystem,
            &DiscoveryEnvironment::from_process_environment(),
        ) {
            Ok(report) => Self::from_retroarch_report(&report),
            Err(_) => Self::default(),
        }
    }
}

pub struct RecoveryContext<'a> {
    pub store: &'a PreparedMediaStore,
    pub trusted: &'a TrustedRoots,
    pub limits: DiscSetLimits,
    pub host: &'a HostFacts,
    pub cancel: &'a AtomicBool,
}

fn extension_of(path: &Path) -> String {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

fn human_bytes(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    if bytes >= 1024 * MIB {
        format!("{:.1} GiB", bytes as f64 / (1024 * MIB) as f64)
    } else if bytes >= MIB {
        format!("{:.0} MiB", bytes as f64 / MIB as f64)
    } else {
        format!("{} KiB", bytes.div_ceil(1024))
    }
}

/// Cheap, pure test a caller may run anywhere (it reads nothing): could this
/// engine have something to say about this file?
pub fn may_apply(path: &Path, platform_id: Option<&str>) -> bool {
    platform_id.is_some_and(|platform| !capability_rows(platform, &extension_of(path)).is_empty())
}

/// True when the engine's validated preparation is the one for this file.
pub fn preparation_may_apply(path: &Path, platform_id: Option<&str>) -> bool {
    platform_id == Some(AMIGA_CD32) && extension_of(path) == "zip"
}

pub fn diagnose(subject: &RecoverySubject, context: &RecoveryContext<'_>) -> RecoveryReport {
    let mut findings = Vec::new();
    let mut plan = None;
    let mut prepared = None;
    let mut damaged_copy = None;
    let mut relinkable = false;

    let source_ok = match std::fs::symlink_metadata(&subject.path) {
        Ok(metadata) if metadata.is_file() || metadata.is_dir() => true,
        Ok(_) => {
            findings.push(source_unavailable(
                "The game's path is a link or a special file; EmuWiz reads game files directly only.",
            ));
            false
        }
        Err(error) => {
            findings.push(source_unavailable(&format!(
                "The game file could not be found ({}).",
                error.kind()
            )));
            false
        }
    };
    if !source_ok {
        return finish(subject, findings, plan, prepared, damaged_copy, relinkable);
    }

    let extension = extension_of(&subject.path);
    let platform = subject.platform_id.as_deref();
    if preparation_may_apply(&subject.path, platform) {
        match inspect_disc_zip(
            &subject.path,
            context.trusted,
            &context.limits,
            context.cancel,
        ) {
            Ok(inspection) => {
                let listed = context.store.lookup(&inspection.fingerprint);
                // A verified copy of this content only serves this ZIP once
                // the store links the two; an identical ZIP elsewhere gets
                // its link from "Prepare", which then extracts nothing.
                let linked = context.store.prepared_for_source(&subject.path);
                let ready = listed
                    .as_ref()
                    .filter(|listed| listed.state == SetState::Ready)
                    .and_then(|listed| listed.summary.clone())
                    .filter(|summary| {
                        linked
                            .as_ref()
                            .is_some_and(|linked| linked.set_id == summary.set_id)
                    });
                let identical_copy_exists = ready.is_none()
                    && listed
                        .as_ref()
                        .is_some_and(|listed| listed.state == SetState::Ready);
                findings.push(disc_set_finding(
                    &inspection,
                    ready.is_some(),
                    identical_copy_exists,
                ));
                relinkable = identical_copy_exists;
                match listed {
                    Some(super::prepared_media::ListedSet {
                        set_id,
                        state,
                        summary,
                    }) => match (state, summary) {
                        (SetState::Ready, Some(summary)) => {
                            if ready.is_some() {
                                findings.push(prepared_ready_finding(&summary));
                            } else {
                                plan = Some(context.store.plan(&inspection));
                            }
                        }
                        (SetState::Damaged(detail), _) => {
                            findings.push(prepared_damaged_finding(&set_id, &detail));
                            damaged_copy = Some(set_id);
                        }
                        (SetState::Ready, None) => {
                            findings.push(prepared_damaged_finding(
                                &set_id,
                                "The receipt could not be summarised",
                            ));
                            damaged_copy = Some(set_id);
                        }
                    },
                    None => plan = Some(context.store.plan(&inspection)),
                }
                let set_id: String = inspection.fingerprint.chars().take(32).collect();
                if let Some(interrupted) = context
                    .store
                    .interrupted()
                    .into_iter()
                    .find(|entry| entry.set_id.as_deref() == Some(set_id.as_str()))
                {
                    findings.push(interrupted_finding(&interrupted.directory));
                }
                findings.push(system_finding(subject, ready.as_ref()));
                findings.extend(cd32_route_findings(context.host, context.cancel));
                prepared = ready;
            }
            Err(refusal) => findings.push(disc_refusal_finding(&refusal)),
        }
    } else if let Some(platform) = platform {
        let rows = capability_rows(platform, &extension);
        if rows.is_empty() {
            findings.push(not_assessed(platform, &extension));
        }
        for row in rows {
            findings.push(capability_finding(row));
        }
    } else {
        findings.push(not_assessed("an unknown system", &extension));
    }
    finish(subject, findings, plan, prepared, damaged_copy, relinkable)
}

/// The text with its first letter in upper case.
fn sentence(text: &str) -> String {
    let mut characters = text.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

fn finish(
    subject: &RecoverySubject,
    findings: Vec<Finding>,
    plan: Option<super::prepared_media::PreparationPlan>,
    prepared: Option<super::prepared_media::PreparedSetSummary>,
    damaged_copy: Option<String>,
    relinkable: bool,
) -> RecoveryReport {
    let has = |code: FindingCode| findings.iter().any(|finding| finding.code == code);
    let person_must_act = findings.iter().any(|finding| {
        finding.blocking == Blocking::Launch
            && matches!(
                finding.code,
                FindingCode::FirmwareMissing | FindingCode::EmulatorRouteMissing
            )
    });
    let verdict = if has(FindingCode::SourceUnavailable) || has(FindingCode::ArchiveRefused) {
        RecoveryVerdict::Refused
    } else if has(FindingCode::FormatUnsupported) || has(FindingCode::DiscSetIncomplete) {
        RecoveryVerdict::Unsupported
    } else if has(FindingCode::DiscSetInsideZip) {
        if person_must_act || has(FindingCode::PreparedCopyDamaged) {
            RecoveryVerdict::FixableWithYourHelp
        } else if prepared.is_some() {
            RecoveryVerdict::PlayableFromPreparedCopy
        } else {
            RecoveryVerdict::FixableByPreparation
        }
    } else {
        RecoveryVerdict::NotAssessed
    };
    RecoveryReport {
        subject: subject.clone(),
        verdict,
        findings,
        plan,
        prepared,
        damaged_copy,
        relinkable,
    }
}

fn source_unavailable(explanation: &str) -> Finding {
    Finding {
        code: FindingCode::SourceUnavailable,
        title: "The game file is not available".into(),
        explanation: explanation.into(),
        confidence: Confidence::Certain,
        blocking: Blocking::Launch,
        evidence: vec![Evidence::new(
            EvidenceSource::FileSystem,
            "Read the path's own type without following links",
        )],
        actions: Vec::new(),
    }
}

fn not_assessed(platform: &str, extension: &str) -> Finding {
    Finding {
        code: FindingCode::NotAssessed,
        title: "No recovery rule covers this file yet".into(),
        explanation: format!(
            "The recovery engine has no reviewed rule for a .{extension} file filed under {platform}. It makes no claim about it; the Launch page's own explanation still applies."
        ),
        confidence: Confidence::Certain,
        blocking: Blocking::Nothing,
        evidence: vec![Evidence::new(
            EvidenceSource::CapabilityMatrix,
            "No row for this system and format",
        )],
        actions: Vec::new(),
    }
}

fn capability_finding(row: &super::capability::CapabilityRow) -> Finding {
    let (code, title, blocking, actions) = match row.status {
        CapabilityStatus::Unsupported => (
            FindingCode::FormatUnsupported,
            "EmuWiz cannot make this file playable",
            Blocking::Launch,
            vec![ProposedAction::new(
                ActionKind::UseAnotherCopy,
                ActionEffect::ReadOnly,
                ActionPerformer::Person,
                "Use another copy or format",
                "Nothing EmuWiz can safely do turns this file into one an installed emulator starts.",
            )],
        ),
        CapabilityStatus::PlayableDirectly => (
            FindingCode::NotAssessed,
            "This format starts as it is",
            Blocking::Nothing,
            Vec::new(),
        ),
        CapabilityStatus::FixableByPreparation | CapabilityStatus::NotAssessed => (
            FindingCode::NotAssessed,
            "Not assessed by the recovery engine",
            Blocking::Nothing,
            Vec::new(),
        ),
    };
    Finding {
        code,
        title: title.into(),
        explanation: format!(
            "{} stored as .{} ({}): {}. {}",
            crate::platform::display_name_for(row.platform_id),
            row.stored_format,
            row.contents,
            row.status.label(),
            row.limits
        ),
        confidence: Confidence::Medium,
        blocking,
        evidence: vec![
            Evidence::new(
                EvidenceSource::Catalogue,
                format!(
                    "Filed under {} with the extension .{}",
                    row.platform_id, row.stored_format
                ),
            ),
            Evidence::new(
                EvidenceSource::CapabilityMatrix,
                format!(
                    "Reviewed rule: emulator route {}; preparation {}",
                    row.emulator_route, row.preparation
                ),
            ),
        ],
        actions,
    }
}

fn disc_set_finding(
    inspection: &DiscSetInspection,
    already_prepared: bool,
    identical_copy_exists: bool,
) -> Finding {
    let tracks = inspection.sheet.track_count();
    let audio = inspection.audio_track_files();
    let mut evidence = vec![
        Evidence::new(
            EvidenceSource::ArchiveIndex,
            format!(
                "The ZIP's index lists {} files for this disc, {} in total",
                inspection.members.len(),
                human_bytes(inspection.total_bytes)
            ),
        ),
        Evidence::new(
            EvidenceSource::MemberContent,
            format!(
                "The cue sheet {} was read from the ZIP and matches its recorded checksum ({})",
                inspection.cue_name(),
                inspection.members[0].crc32
            ),
        ),
        Evidence::new(
            EvidenceSource::CueSheet,
            format!(
                "The cue sheet describes {tracks} track(s) in {} file(s), {audio} of them audio; every file it names is in the ZIP",
                inspection.sheet.files.len()
            ),
        ),
    ];
    if !inspection.ignored_members.is_empty() {
        evidence.push(Evidence::new(
            EvidenceSource::ArchiveIndex,
            format!(
                "{} other file(s) in the ZIP are not part of the disc and are left alone",
                inspection.ignored_members.len()
            ),
        ));
    }
    for irregularity in &inspection.irregularities {
        evidence.push(Evidence::new(
            EvidenceSource::ArchiveIndex,
            format!("Irregular: {irregularity}"),
        ));
    }
    let actions = if already_prepared {
        Vec::new()
    } else if identical_copy_exists {
        evidence.push(Evidence::new(
            EvidenceSource::PreparedReceipt,
            "A prepared copy with the same file names, sizes and ZIP checksums already exists (prepared from another ZIP)",
        ));
        vec![ProposedAction::new(
            ActionKind::PrepareDiscSetFromZip,
            ActionEffect::CreatesNewFiles,
            ActionPerformer::Engine,
            "Use the existing prepared copy",
            "Reads this ZIP in full and compares every file with the copy already prepared (SHA-256). If they match, records that this ZIP may use that copy. Nothing is extracted again; the ZIP is only read.",
        )]
    } else {
        vec![ProposedAction::new(
            ActionKind::PrepareDiscSetFromZip,
            ActionEffect::CreatesNewFiles,
            ActionPerformer::Engine,
            "Prepare a playable copy",
            format!(
                "Extracts the {} disc files into EmuWiz's prepared-media folder as new files ({}), checks each against the ZIP's checksum and reads them back. The ZIP is only read.",
                inspection.members.len(),
                human_bytes(inspection.total_bytes)
            ),
        )]
    };
    Finding {
        code: FindingCode::DiscSetInsideZip,
        title: "This CD game is stored inside a ZIP".into(),
        explanation:
            "The emulator needs the disc's cue sheet and track files as ordinary files next to each other. It cannot start them from inside a ZIP."
                .into(),
        confidence: if inspection.irregularities.is_empty() {
            Confidence::High
        } else {
            Confidence::Medium
        },
        blocking: if already_prepared {
            Blocking::Nothing
        } else {
            Blocking::Launch
        },
        evidence,
        actions,
    }
}

fn disc_refusal_finding(refusal: &DiscSetRefusal) -> Finding {
    let (code, title, blocking) = match refusal {
        DiscSetRefusal::Source(_) => (
            FindingCode::SourceUnavailable,
            "The game file cannot be read safely",
            Blocking::Launch,
        ),
        DiscSetRefusal::Archive(_)
        | DiscSetRefusal::Limit(_)
        | DiscSetRefusal::UnsafeMember(_)
        | DiscSetRefusal::Cancelled => (
            FindingCode::ArchiveRefused,
            "EmuWiz will not extract this ZIP",
            Blocking::Preparation,
        ),
        DiscSetRefusal::NoCueSheet
        | DiscSetRefusal::MultipleCueSheets(_)
        | DiscSetRefusal::NestedFolders
        | DiscSetRefusal::Cue(_)
        | DiscSetRefusal::MissingTracks(_)
        | DiscSetRefusal::CaseMismatch { .. } => (
            FindingCode::DiscSetIncomplete,
            "The ZIP does not hold one complete disc this engine can prepare",
            Blocking::Preparation,
        ),
    };
    Finding {
        code,
        title: title.into(),
        explanation: format!(
            "{}. Nothing was extracted and nothing was changed.",
            sentence(&refusal.to_string())
        ),
        confidence: Confidence::Certain,
        blocking,
        evidence: vec![Evidence::new(
            EvidenceSource::ArchiveIndex,
            "Read the ZIP's index and cue sheet only, within fixed size limits",
        )],
        actions: vec![ProposedAction::new(
            ActionKind::UseAnotherCopy,
            ActionEffect::ReadOnly,
            ActionPerformer::Person,
            "Use another copy, or extract it yourself",
            "EmuWiz only prepares a ZIP whose cue sheet and tracks it can verify completely.",
        )],
    }
}

fn prepared_ready_finding(summary: &super::prepared_media::PreparedSetSummary) -> Finding {
    let mut evidence = vec![Evidence::new(
        EvidenceSource::PreparedReceipt,
        format!(
            "A receipt records {} files ({}) with checksums; each file still has the recorded size and timestamps",
            summary.file_count,
            human_bytes(summary.total_bytes)
        ),
    )];
    if let Some(disc) = &summary.disc {
        evidence.push(Evidence::new(
            EvidenceSource::DiscStructure,
            format!(
                "The data track is an ISO 9660 disc; system identifier \"{}\", volume \"{}\"",
                disc.system_identifier, disc.volume_identifier
            ),
        ));
    }
    Finding {
        code: FindingCode::PreparedCopyReady,
        title: "A verified copy of the disc files is ready".into(),
        explanation: format!(
            "EmuWiz prepared this disc earlier. The game starts from {}.",
            summary.launch_path.display()
        ),
        confidence: Confidence::High,
        blocking: Blocking::Nothing,
        evidence,
        actions: vec![
            ProposedAction::new(
                ActionKind::PlayPreparedCopy,
                ActionEffect::ReadOnly,
                ActionPerformer::Person,
                "Play from the game's Launch page",
                "Play starts the prepared copy. The Launch page still asks you to confirm the system and checks the emulator, its firmware and the prepared files.",
            ),
            ProposedAction::new(
                ActionKind::VerifyPreparedCopy,
                ActionEffect::ReadOnly,
                ActionPerformer::Engine,
                "Check the prepared copy again",
                "Reads every prepared file and compares it with the checksum in the receipt.",
            ),
            ProposedAction::new(
                ActionKind::RemovePreparedCopy,
                ActionEffect::ChangesExistingState,
                ActionPerformer::Engine,
                "Remove the prepared copy",
                "Deletes only the files EmuWiz created for this disc. The ZIP is not touched.",
            ),
        ],
    }
}

fn prepared_damaged_finding(set_id: &str, detail: &str) -> Finding {
    Finding {
        code: FindingCode::PreparedCopyDamaged,
        title: "The prepared copy no longer passes its checks".into(),
        explanation: format!(
            "{}. EmuWiz will not start it and will not overwrite it. Remove it, then prepare it again.",
            sentence(detail)
        ),
        confidence: Confidence::Certain,
        blocking: Blocking::Launch,
        evidence: vec![Evidence::new(
            EvidenceSource::PreparedReceipt,
            format!("Compared prepared copy {set_id} with its receipt"),
        )],
        actions: vec![ProposedAction::new(
            ActionKind::RemovePreparedCopy,
            ActionEffect::ChangesExistingState,
            ActionPerformer::Engine,
            "Remove the prepared copy",
            "Deletes only files the receipt lists that still have the recorded size; anything else is left and reported.",
        )],
    }
}

fn interrupted_finding(directory: &Path) -> Finding {
    Finding {
        code: FindingCode::PreparationInterrupted,
        title: "An earlier preparation did not finish".into(),
        explanation:
            "Unfinished files are kept apart and are never used to start a game. They can be cleaned up safely."
                .into(),
        confidence: Confidence::Certain,
        blocking: Blocking::Nothing,
        evidence: vec![Evidence::new(
            EvidenceSource::FileSystem,
            format!("Work folder {}", directory.display()),
        )],
        actions: vec![ProposedAction::new(
            ActionKind::RemovePreparedCopy,
            ActionEffect::ChangesExistingState,
            ActionPerformer::Engine,
            "Clean up the unfinished preparation",
            "Deletes only the files that preparation's own record lists.",
        )],
    }
}

fn system_finding(
    subject: &RecoverySubject,
    prepared: Option<&super::prepared_media::PreparedSetSummary>,
) -> Finding {
    let source = subject.platform_source.as_deref();
    let folder_only = !matches!(source, Some("manual"));
    let mut evidence = vec![Evidence::new(
        EvidenceSource::Catalogue,
        match source {
            Some(source) => {
                format!("The library records Amiga CD32 for this file (source: {source})")
            }
            None => "The library records Amiga CD32 for this file".to_string(),
        },
    )];
    let mut confidence = if folder_only {
        Confidence::Low
    } else {
        Confidence::Medium
    };
    if let Some(disc) = prepared.and_then(|summary| summary.disc.as_ref()) {
        if disc.system_identifier.eq_ignore_ascii_case("CDTV") {
            evidence.push(Evidence::new(
                EvidenceSource::DiscStructure,
                "The disc's own system identifier is \"CDTV\", which Commodore CDTV and CD32 discs both carry; it does not tell the two apart",
            ));
            confidence = confidence.max(Confidence::Medium);
        } else {
            evidence.push(Evidence::new(
                EvidenceSource::DiscStructure,
                format!(
                    "The disc's own system identifier is \"{}\", which does not name a Commodore CD system",
                    disc.system_identifier
                ),
            ));
        }
    }
    Finding {
        code: FindingCode::SystemFromFolderOnly,
        title: if folder_only {
            "The system is known only from the folder".into()
        } else {
            "You chose this game's system".into()
        },
        explanation:
            "The game itself is not identified by a catalogue. EmuWiz never treats a folder name as proof, so the Launch page asks you to confirm before the first start (\"Try anyway\" saves nothing)."
                .into(),
        confidence,
        blocking: Blocking::Nothing,
        evidence,
        actions: vec![ProposedAction::new(
            ActionKind::ConfirmSystem,
            ActionEffect::ReadOnly,
            ActionPerformer::Person,
            "Confirm the system on the Launch page",
            "\"Try anyway\" starts the game once and saves nothing; \"Remember this launch choice\" saves your choice for this unchanged file.",
        )],
    }
}

/// SHA-1 of a small firmware file, read through at most one symlink-free
/// open of the resolved file (firmware folders commonly hold links to the
/// person's own BIOS collection, so the link is followed deliberately).
fn firmware_sha1(path: &Path, cancel: &AtomicBool) -> Option<(u64, String)> {
    const MAX_FIRMWARE_BYTES: u64 = 4 * 1024 * 1024;
    let file = crate::safe_read::open_regular_file_nonblocking(path, true).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_FIRMWARE_BYTES {
        return None;
    }
    let hashed = crate::dat::archive::hash::hash_member_stream(
        file.take(MAX_FIRMWARE_BYTES),
        MAX_FIRMWARE_BYTES,
        cancel,
    )
    .ok()?;
    Some((hashed.bytes_read, hashed.hashes.sha1))
}

fn cd32_route_findings(host: &HostFacts, cancel: &AtomicBool) -> Vec<Finding> {
    let mut findings = Vec::new();
    let with_core: Vec<&RetroArchHost> = host
        .retroarch
        .iter()
        .filter(|installation| {
            installation
                .core_stems
                .iter()
                .any(|stem| stem == CD32_CORE_STEM)
        })
        .collect();
    let Some(installation) = with_core.first() else {
        findings.push(Finding {
            code: FindingCode::EmulatorRouteMissing,
            title: "No installed emulator can run CD32 discs".into(),
            explanation: if host.retroarch_discovered {
                "EmuWiz starts CD32 discs with RetroArch's PUAE core. No RetroArch installation EmuWiz can start has that core.".into()
            } else {
                "EmuWiz could not look for RetroArch on this machine, so it cannot tell whether the PUAE core is installed.".into()
            },
            confidence: if host.retroarch_discovered {
                Confidence::High
            } else {
                Confidence::Low
            },
            blocking: Blocking::Launch,
            evidence: vec![Evidence::new(
                EvidenceSource::EmulatorInventory,
                format!(
                    "{} RetroArch installation(s) found; none lists the core \"{CD32_CORE_STEM}\"",
                    host.retroarch.len()
                ),
            )],
            actions: vec![ProposedAction::new(
                ActionKind::InstallEmulatorCore,
                ActionEffect::ChangesExistingState,
                ActionPerformer::Person,
                "Install RetroArch's PUAE core",
                "Install it with RetroArch's own Core Downloader or your package manager. EmuWiz does not download it for you here.",
            )],
        });
        return findings;
    };
    findings.push(Finding {
        code: FindingCode::EmulatorRouteAvailable,
        title: "RetroArch's PUAE core is installed".into(),
        explanation: "EmuWiz starts CD32 discs with this core.".into(),
        confidence: Confidence::High,
        blocking: Blocking::Nothing,
        evidence: vec![Evidence::new(
            EvidenceSource::EmulatorInventory,
            format!("{} lists the core \"{CD32_CORE_STEM}\"", installation.label),
        )],
        actions: Vec::new(),
    });

    let Some(system) = installation.system_directory.as_deref() else {
        findings.push(Finding {
            code: FindingCode::FirmwareMissing,
            title: "RetroArch's system folder is not known".into(),
            explanation:
                "The CD32 needs its Kickstart and extended ROM in RetroArch's system folder, and that folder could not be worked out from RetroArch's settings."
                    .into(),
            confidence: Confidence::Medium,
            blocking: Blocking::Launch,
            evidence: vec![Evidence::new(
                EvidenceSource::EmulatorInventory,
                "RetroArch's settings give no usable system folder",
            )],
            actions: Vec::new(),
        });
        return findings;
    };
    let mut missing = Vec::new();
    let mut evidence = Vec::new();
    let mut all_known = true;
    for firmware in CD32_FIRMWARE {
        match firmware_sha1(&system.join(firmware.file_name), cancel) {
            None => {
                all_known = false;
                missing.push(firmware);
                evidence.push(Evidence::new(
                    EvidenceSource::FirmwareFile,
                    format!(
                        "{} ({}) is not in {}",
                        firmware.file_name,
                        firmware.description,
                        system.display()
                    ),
                ));
            }
            Some((size, sha1)) if sha1.eq_ignore_ascii_case(firmware.sha1) => {
                evidence.push(Evidence::new(
                    EvidenceSource::FirmwareFile,
                    format!(
                        "{} is present ({size} bytes) and its SHA-1 is the known {}",
                        firmware.file_name, firmware.description
                    ),
                ));
            }
            Some((size, sha1)) => {
                all_known = false;
                evidence.push(Evidence::new(
                    EvidenceSource::FirmwareFile,
                    format!(
                        "{} is present ({size} bytes) but its SHA-1 {sha1} is not the known {}; it may still work",
                        firmware.file_name, firmware.description
                    ),
                ));
            }
        }
    }
    if missing.is_empty() {
        findings.push(Finding {
            code: FindingCode::FirmwarePresent,
            title: "The CD32 firmware is in place".into(),
            explanation: if all_known {
                "Both files are the known releases.".into()
            } else {
                "Both files are present; at least one is not a release this engine knows.".into()
            },
            confidence: if all_known {
                Confidence::Certain
            } else {
                Confidence::Medium
            },
            blocking: Blocking::Nothing,
            evidence,
            actions: Vec::new(),
        });
    } else {
        findings.push(Finding {
            code: FindingCode::FirmwareMissing,
            title: "The CD32 firmware is missing".into(),
            explanation: format!(
                "RetroArch's PUAE core needs {} in {}. Without them the CD32 does not start.",
                missing
                    .iter()
                    .map(|firmware| firmware.file_name)
                    .collect::<Vec<_>>()
                    .join(" and "),
                system.display()
            ),
            confidence: Confidence::High,
            blocking: Blocking::Launch,
            evidence,
            actions: vec![ProposedAction::new(
                ActionKind::AddOwnFirmware,
                ActionEffect::ChangesExistingState,
                ActionPerformer::Person,
                "Add your own firmware files",
                "Copy the Kickstart and extended ROM dumped from your own CD32 into RetroArch's system folder under exactly these names. EmuWiz never downloads firmware.",
            )],
        });
    }
    findings
}
