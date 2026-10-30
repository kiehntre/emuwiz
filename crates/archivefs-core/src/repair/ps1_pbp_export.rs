//! Opt-in PS1 playing-copy export. This never grants launch authority or moves originals.
//! The only publication path is the existing no-clobber, journaled Repair move.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::dat::library_identity_summary::{
    DatProvenanceFreshness, DatVerificationState, DiscFileState, LibraryItemHashes,
};
use crate::dat::rename_apply::identity::capture_identity;
use crate::dat::rename_apply::model::TransactionState;
use crate::dat::sources::DatSourceEntry;
use crate::dat::sources::now_unix;
use crate::database::Database;
use crate::game_identity::inspect_game_identity;
use crate::ingestion::cue_bin::{CueDataTrackMode, CueTrackMode, resolve_cue_layout};
use crate::ingestion::ps1_cue_disc::inspect_ps1_cue_disc;
use crate::launch::media_handoff::verified_media;
use crate::media_set::dat_disc_order::{
    dat_disc_catalogue_sha256, dat_disc_claim, dat_disc_token, is_dat_disc_order_set,
};
use crate::media_set::{
    MediaFamily, MediaProfile, MediaSet, MediaSetConfidence, MediaSetState, OrdinalUnit,
};
use crate::repair::execute::{
    RepairApplyExecution, RepairExecutionOptions, RepairReverifyOutcome, RepairTransactionResult,
    apply_repair_transaction, build_repair_transaction, rollback_repair_transaction,
};
use crate::repair::plan::{RepairPlanId, build_repair_plan};
use crate::repair::proposal::{
    RepairAction, RepairEvidence, RepairEvidenceKind, RepairProposal, RepairProposalId, SafetyState,
};
use crate::safe_read::TrustedRoots;

/// v1.6.3 release archive, x86-64 Linux. The executable itself reports 1.6.2;
/// both its bytes and that exact runtime string are checked. The archive is
/// user-installed: its bundled BASE.PBP/KEYS.BIN are not redistributed here.
pub const WRITER_RELEASE: &str = "PSXPackager v1.6.3 linux-x64";
pub const WRITER_SHA256: &str = "2ab8477755b6307dbf4f1c5af496245b720a61c289f9056a7d1d58375a0b000f";
pub const WRITER_ARCHIVE_SHA256: &str =
    "378ecb4d2310c8f72fc77ec092e50797427de79d6e001ab0b56eaad4699890da";
const WRITER_RUNTIME_VERSION: &str = "psxpackager 1.6.2";
const WRITER_RESOURCES: [(&str, &str); 3] = [
    (
        "BASE.PBP",
        "e0d49b137102eea2d50a9cfc8359e9cd835d10c3f657b59fc0f086b4671cec3d",
    ),
    (
        "KEYS.BIN",
        "acfff0dd101778113a28bf3a77f361c3510cd9aace50927e2ad5d4fea6435f2e",
    ),
    (
        "gameInfo.db",
        "f412d31d59a8e9f618667769e1ce13590276a40f12caefbb459501713101ac60",
    ),
];
const PROCESS_LIMIT: Duration = Duration::from_secs(30 * 60);
pub const DERIVED_LABEL: &str = "Derived playing copy: retain original discs";
pub const SUPPORTED_INPUTS: &str = "2–5 verified PS1 discs; one raw MODE2/2352 BIN/CUE track per disc, INDEX 01 00:00:00, no gaps, audio or extra indexes";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PbpExportError {
    pub detail: String,
    pub recovery: Option<PbpRecoveryStatus>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PbpRecoveryStatus {
    Unknown,
    NothingPublished {
        transaction_id: String,
        stage: PathBuf,
    },
    RolledBack {
        transaction_id: String,
        stage: PathBuf,
    },
    NeedsReview {
        transaction_id: String,
        stage: PathBuf,
        detail: String,
    },
}
impl std::fmt::Display for PbpExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}
impl std::error::Error for PbpExportError {}
fn refuse(detail: impl Into<String>) -> PbpExportError {
    PbpExportError {
        detail: detail.into(),
        recovery: None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PbpExportProgress {
    Rechecking,
    Copying { disc: u16, total: usize },
    Packaging,
    Extracting,
    Comparing { disc: u16, total: usize },
    Publishing,
    Recovering,
}

#[derive(Debug, Clone)]
pub struct PbpDiscProof {
    ordinal: u16,
    cue: PathBuf,
    bin: PathBuf,
    cue_sha256: String,
    bin_sha256: String,
    serial: String,
    dat_entry: String,
}
impl PbpDiscProof {
    pub fn ordinal(&self) -> u16 {
        self.ordinal
    }
    pub fn cue(&self) -> &Path {
        &self.cue
    }
    pub fn bin(&self) -> &Path {
        &self.bin
    }
    pub fn serial(&self) -> &str {
        &self.serial
    }
    pub fn dat_entry(&self) -> &str {
        &self.dat_entry
    }
    pub fn cue_sha256(&self) -> &str {
        &self.cue_sha256
    }
    pub fn bin_sha256(&self) -> &str {
        &self.bin_sha256
    }
}

#[derive(Debug, Clone)]
pub struct PbpExportPlan {
    set: MediaSet,
    discs: Vec<PbpDiscProof>,
    region: String,
    dat_source: String,
    dat_revision: String,
    dat_catalogue_sha256: String,
    destination: PathBuf,
    receipt_path: PathBuf,
    writer: PathBuf,
}
impl PbpExportPlan {
    pub fn destination(&self) -> &Path {
        &self.destination
    }
    pub fn disc_count(&self) -> usize {
        self.discs.len()
    }
    pub fn region(&self) -> &str {
        &self.region
    }
    pub fn discs(&self) -> &[PbpDiscProof] {
        &self.discs
    }
    pub fn receipt_path(&self) -> &Path {
        &self.receipt_path
    }
    pub fn release(&self) -> &str {
        &self.set.identity.key.value
    }
    pub fn dat_source(&self) -> &str {
        &self.dat_source
    }
    pub fn dat_revision(&self) -> &str {
        &self.dat_revision
    }
    pub fn dat_catalogue_sha256(&self) -> &str {
        &self.dat_catalogue_sha256
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PbpExportDiscReceipt {
    pub ordinal: u16,
    pub cue: PathBuf,
    pub bin: PathBuf,
    pub cue_sha256: String,
    pub bin_sha256: String,
    pub serial: String,
    pub dat_entry: String,
    pub roundtrip_bin_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PbpExportReceipt {
    pub schema: String,
    pub label: String,
    pub release: String,
    pub region: String,
    pub dat_source: String,
    pub dat_revision: String,
    pub dat_catalogue_sha256: String,
    pub writer_release: String,
    pub writer_sha256: String,
    pub pbp_sha256: String,
    pub discs: Vec<PbpExportDiscReceipt>,
}

#[derive(Debug)]
pub struct PbpExportResult {
    pub receipt: PbpExportReceipt,
    pub transaction: RepairTransactionResult,
    /// Retained private directory: the journal needs its source paths for Undo.
    pub recovery_stage: PathBuf,
    /// Non-fatal cleanup error after confirmed publication.
    pub cleanup_warning: Option<String>,
}

fn sha256(path: &Path, cancel: &AtomicBool) -> Result<String, PbpExportError> {
    let mut file = File::open(path).map_err(|e| refuse(format!("{}: {e}", path.display())))?;
    let mut hash = Sha256::new();
    let mut buf = [0u8; 256 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(refuse("export cancelled"));
        }
        let count = file.read(&mut buf).map_err(|e| refuse(e.to_string()))?;
        if count == 0 {
            break;
        }
        hash.update(&buf[..count]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn region_of(name: &str) -> Option<&'static str> {
    let regions = [
        ("(USA)", "USA"),
        ("(Europe)", "Europe"),
        ("(Japan)", "Japan"),
    ];
    let mut found = None;
    for (token, region) in regions {
        if name.contains(token) {
            if found.is_some() {
                return None;
            }
            found = Some(region);
        }
    }
    found
}
fn serial_region(serial: &str) -> Option<&'static str> {
    let prefix = serial.split('-').next()?;
    match prefix {
        "SLUS" | "SCUS" => Some("USA"),
        "SLES" | "SCES" => Some("Europe"),
        "SLPS" | "SCPS" | "SLPM" | "SCPM" => Some("Japan"),
        _ => None,
    }
}

fn writer_check(path: &Path, cancel: &AtomicBool) -> Result<(), PbpExportError> {
    let metadata = fs::symlink_metadata(path).map_err(|e| refuse(e.to_string()))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || sha256(path, cancel)? != WRITER_SHA256
    {
        return Err(refuse(
            "PSXPackager executable is not the pinned Linux build",
        ));
    }
    let resources = path
        .parent()
        .ok_or_else(|| refuse("writer has no parent"))?
        .join("Resources");
    for (name, expected) in WRITER_RESOURCES {
        let file = resources.join(name);
        let meta = fs::symlink_metadata(&file).map_err(|e| refuse(e.to_string()))?;
        if !meta.is_file() || meta.file_type().is_symlink() || sha256(&file, cancel)? != expected {
            return Err(refuse(format!(
                "writer resource {name} differs from pinned release"
            )));
        }
    }
    let output = Command::new(path)
        .arg("--version")
        .output()
        .map_err(|e| refuse(format!("cannot run pinned writer: {e}")))?;
    if !output.status.success()
        || !String::from_utf8_lossy(&output.stderr).contains(WRITER_RUNTIME_VERSION)
    {
        return Err(refuse(
            "PSXPackager runtime version differs from pinned release artifact",
        ));
    }
    Ok(())
}

fn profile() -> MediaProfile {
    MediaProfile {
        id: "ps1-pbp-export-v1".into(),
        platform: "PSX".into(),
        supported_formats: ["cue".into()].into(),
        preferred_formats: vec!["cue".into()],
        readiness_hint: None,
    }
}

fn accepted_layout(cue: &Path, bin: &Path) -> Result<(), PbpExportError> {
    let layout = resolve_cue_layout(cue).map_err(|e| refuse(e.to_string()))?;
    if layout.tracks.len() != 1 {
        return Err(refuse("requires exactly one track"));
    }
    let track = &layout.tracks[0];
    if track.number != 1
        || track.path != bin
        || track.file_type != "BINARY"
        || !matches!(track.mode, CueTrackMode::Data(CueDataTrackMode::Mode2_2352))
        || track.index_01.is_none_or(|i| i.frames != 0)
        || track.index_00.is_some()
        || track.pregap.is_some()
        || track.postgap.is_some()
        || !track.extra_indexes.is_empty()
    {
        return Err(refuse(
            "Phase 1 supports one raw MODE2/2352 track at INDEX 01 00:00:00, with no gaps or extra indexes",
        ));
    }
    Ok(())
}

/// Read-only admission. DAT source/revision and whole-disc file receipts are
/// checked against the current database, then every CUE and BIN is hashed now.
pub fn plan_ps1_pbp_export(
    db: &Database,
    sources: &[DatSourceEntry],
    set: &MediaSet,
    destination: &Path,
    writer: &Path,
    cancel: &AtomicBool,
) -> Result<PbpExportPlan, PbpExportError> {
    if set.state != MediaSetState::CompleteSet
        || set.confidence != MediaSetConfidence::Proven
        || set.platform.as_deref() != Some("PSX")
        || set.family != Some(MediaFamily::Optical)
        || !is_dat_disc_order_set(set)
        || !set.identity.verified
        || !set.conflicts.is_empty()
    {
        return Err(refuse(
            "requires one current, proven PS1 DAT-ordered CompleteSet",
        ));
    }
    let region = region_of(&set.identity.key.value)
        .ok_or_else(|| refuse("DAT release has no unambiguous supported region"))?;
    if set
        .variant
        .region
        .as_deref()
        .is_some_and(|value| value != region)
    {
        return Err(refuse("release region conflicts with DAT identity"));
    }
    let (swap, paths, family) = verified_media(set, &profile()).map_err(|e| refuse(e.detail))?;
    if family != MediaFamily::Optical
        || !(2..=5).contains(&paths.len())
        || paths.len() != set.members.len()
        || swap.ordered_media.len() != paths.len()
    {
        return Err(refuse(
            "PBP export requires two through five complete discs",
        ));
    }
    if set
        .expected_count
        .as_ref()
        .is_none_or(|(count, provenance)| {
            count.count as usize != paths.len()
                || count.unit != OrdinalUnit::Disc
                || !provenance.trusted()
        })
    {
        return Err(refuse("the DAT does not prove the complete disc count"));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| refuse("destination has no parent"))?;
    if !destination.is_absolute()
        || destination.extension().and_then(|x| x.to_str()) != Some("pbp")
        || !parent.is_dir()
        || fs::symlink_metadata(destination).is_ok()
    {
        return Err(refuse(
            "destination must be a new absolute .pbp path in an existing folder",
        ));
    }
    let receipt_path = destination.with_extension("pbp.receipt.json");
    if fs::symlink_metadata(&receipt_path).is_ok() {
        return Err(refuse("verification receipt destination exists"));
    }
    writer_check(writer, cancel)?;
    let archives = db.load_archives().map_err(|e| refuse(e.to_string()))?;
    let by_path: BTreeMap<_, _> = archives
        .iter()
        .map(|a| (a.absolute_path.as_path(), a))
        .collect();
    let mut discs = Vec::with_capacity(paths.len());
    let mut common: Option<(String, String, String)> = None;
    for (index, (step, cue)) in swap.ordered_media.iter().zip(&paths).enumerate() {
        let ordinal = u16::try_from(index + 1).map_err(|e| refuse(e.to_string()))?;
        if step
            .ordinal
            .as_ref()
            .is_none_or(|o| o.unit != OrdinalUnit::Disc || o.number != ordinal)
        {
            return Err(refuse(
                "disc ordinals are missing, duplicate, or out of order",
            ));
        }
        let representation = set
            .members
            .iter()
            .flat_map(|m| &m.representations)
            .find(|r| r.record.source.path == *cue && r.record.source.archive_member.is_none())
            .ok_or_else(|| refuse("selected disc has no release representation"))?;
        let (source, revision, entry) =
            dat_disc_claim(representation).ok_or_else(|| refuse("disc lacks trusted DAT claim"))?;
        let token = dat_disc_token(&entry).ok_or_else(|| refuse("DAT disc order is invalid"))?;
        if token.number != ordinal
            || token.unit != OrdinalUnit::Disc
            || token.base_title != set.identity.key.value
            || region_of(&entry) != Some(region)
        {
            return Err(refuse(
                "disc DAT release, order, or region disagrees with set",
            ));
        }
        let digest = dat_disc_catalogue_sha256(representation)
            .ok_or_else(|| refuse("disc DAT evidence lacks exact catalogue digest"))?
            .to_owned();
        let configured = sources
            .iter()
            .find(|item| {
                item.id == source
                    && item.enabled
                    && item.kind == crate::dat::sources::DatSourceKind::File
            })
            .ok_or_else(|| refuse("the DAT source is no longer configured and enabled"))?;
        if common
            .as_ref()
            .is_some_and(|c| c != &(source.clone(), revision.clone(), digest.clone()))
        {
            return Err(refuse("discs are from different DAT sources or revisions"));
        }
        common = Some((source.clone(), revision.clone(), digest.clone()));
        let inventory = db
            .expected_dat_inventory_meta(&source)
            .map_err(|e| refuse(e.to_string()))?
            .ok_or_else(|| refuse("DAT inventory is unavailable"))?;
        let inventory_digest = db
            .expected_dat_inventory_sha256(&source)
            .map_err(|e| refuse(e.to_string()))?;
        if inventory.source_revision.as_deref() != Some(&revision)
            || inventory_digest.as_deref() != Some(&digest)
            || inventory.duplicate_names_skipped != 0
        {
            return Err(refuse("DAT inventory changed or is ambiguous"));
        }
        let archive = by_path
            .get(cue.as_path())
            .ok_or_else(|| refuse("disc is absent from library"))?;
        let summary = db
            .library_dat_identity_summary_for_item(
                archive.id,
                &source,
                Some(&LibraryItemHashes {
                    size_bytes: archive.size_bytes,
                    ..Default::default()
                }),
                Some(&revision),
                true,
            )
            .map_err(|e| refuse(e.to_string()))?
            .ok_or_else(|| refuse("DAT identity is missing"))?;
        if !summary.is_verified()
            || summary.provenance_freshness != DatProvenanceFreshness::Current
            || summary.canonical.canonical_dat_name.as_deref() != Some(&entry)
            || summary.source.catalogue_sha256.as_deref() != Some(&digest)
            || !matches!(
                summary.verification_state,
                DatVerificationState::VerifiedSingleMatch { .. }
            )
        {
            return Err(refuse("disc DAT verification is stale or uncertain"));
        }
        let saved = db
            .library_dat_identity_for_item(archive.id, &source)
            .map_err(|e| refuse(e.to_string()))?
            .ok_or_else(|| refuse("whole-disc audit is missing"))?;
        if configured.path.as_path() != Path::new(&saved.source.dat_path)
            || sha256(&configured.path, cancel)? != digest
        {
            return Err(refuse("configured DAT file differs from audited catalogue"));
        }
        let inspected = inspect_ps1_cue_disc(cue).map_err(|e| refuse(e.to_string()))?;
        if inspected.track_count != 1
            || inspected.companions.len() != 1
            || saved.disc_members.len() != 2
            || !saved.disc_members_unchanged()
        {
            return Err(refuse(
                "disc is not a current, single-track whole-disc BIN/CUE audit",
            ));
        }
        let bin = inspected.companions[0].clone();
        if !saved
            .disc_members
            .iter()
            .any(|f| f.path == *cue && DiscFileState::of(f) == DiscFileState::Unchanged)
            || !saved
                .disc_members
                .iter()
                .any(|f| f.path == bin && DiscFileState::of(f) == DiscFileState::Unchanged)
        {
            return Err(refuse("CUE or BIN differs from its whole-disc audit"));
        }
        accepted_layout(cue, &bin)?;
        let serial = inspect_game_identity(cue, Some("PSX"))
            .verified_ps1_serial()
            .ok_or_else(|| refuse("source PS1 serial is not verified"))?
            .to_owned();
        if serial_region(&serial) != Some(region) {
            return Err(refuse("PS1 serial region conflicts with DAT release"));
        }
        discs.push(PbpDiscProof {
            ordinal,
            cue: cue.clone(),
            bin: bin.clone(),
            cue_sha256: sha256(cue, cancel)?,
            bin_sha256: sha256(&bin, cancel)?,
            serial,
            dat_entry: entry,
        });
    }
    let (dat_source, dat_revision, dat_catalogue_sha256) =
        common.ok_or_else(|| refuse("empty set"))?;
    Ok(PbpExportPlan {
        set: set.clone(),
        discs,
        region: region.into(),
        dat_source,
        dat_revision,
        dat_catalogue_sha256,
        destination: destination.into(),
        receipt_path,
        writer: writer.into(),
    })
}

fn recheck(plan: &PbpExportPlan, cancel: &AtomicBool) -> Result<(), PbpExportError> {
    for disc in &plan.discs {
        let current = inspect_ps1_cue_disc(&disc.cue).map_err(|e| refuse(e.to_string()))?;
        if current.track_count != 1
            || current.companions != vec![disc.bin.clone()]
            || sha256(&disc.cue, cancel)? != disc.cue_sha256
            || sha256(&disc.bin, cancel)? != disc.bin_sha256
        {
            return Err(refuse(format!("disc {} source changed", disc.ordinal)));
        }
    }
    Ok(())
}

fn run_writer(
    writer: &Path,
    args: &[&Path],
    stage: &Path,
    cancel: &AtomicBool,
) -> Result<(), PbpExportError> {
    let log = File::create(stage.join("writer.log")).map_err(|e| refuse(e.to_string()))?;
    let mut command = Command::new(writer);
    for arg in args {
        command.arg(arg);
    }
    let mut child = command
        .current_dir(stage)
        .stdout(Stdio::from(
            log.try_clone().map_err(|e| refuse(e.to_string()))?,
        ))
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|e| refuse(format!("writer failed to start: {e}")))?;
    let start = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) || start.elapsed() > PROCESS_LIMIT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(refuse("writer cancelled or exceeded time limit"));
        }
        match child.try_wait().map_err(|e| refuse(e.to_string()))? {
            Some(status) if status.success() => return Ok(()),
            Some(status) => return Err(refuse(format!("writer exited with {status}"))),
            None => thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn proposal(
    id: String,
    source: PathBuf,
    destination: PathBuf,
) -> Result<RepairProposal, PbpExportError> {
    let identity = capture_identity(&source).map_err(|e| refuse(e.to_string()))?;
    Ok(RepairProposal {
        id: RepairProposalId::new(id).ok_or_else(|| refuse("unsafe proposal id"))?,
        action: RepairAction::MovePath { destination },
        source_path: source,
        reason: DERIVED_LABEL.into(),
        evidence: vec![RepairEvidence::new(
            RepairEvidenceKind::UserRequestedOrganisation,
            "all PBP discs round-trip verified",
        )],
        expected_source_identity: Some(identity),
        originating_audit: None,
        safety: SafetyState::Safe,
        blockers: Vec::new(),
        warnings: Vec::new(),
        dat_source_id: None,
        dat_source_display: None,
        game_name: None,
        rom_name: None,
        verdict_label: Some(DERIVED_LABEL.into()),
        match_confident: true,
        is_outer_archive: false,
        is_outer_archive_verified: false,
        survivor_path: None,
    })
}

/// Re-checks live DAT authority and every original before and after conversion.
/// The writer only sees private copies. A private stage is retained from the
/// first journal checkpoint so an interrupted publication remains recoverable.
pub fn execute_ps1_pbp_export(
    db: &Database,
    sources: &[DatSourceEntry],
    plan: &PbpExportPlan,
    journal_dir: &Path,
    cancel: &AtomicBool,
) -> Result<PbpExportResult, PbpExportError> {
    execute_ps1_pbp_export_with_progress(db, sources, plan, journal_dir, cancel, &|_| {})
}

/// The same fail-closed executor with read-only progress notifications for a
/// background UI worker. Notifications never authorize or alter a transaction.
pub fn execute_ps1_pbp_export_with_progress(
    db: &Database,
    sources: &[DatSourceEntry],
    plan: &PbpExportPlan,
    journal_dir: &Path,
    cancel: &AtomicBool,
    progress: &dyn Fn(PbpExportProgress),
) -> Result<PbpExportResult, PbpExportError> {
    progress(PbpExportProgress::Rechecking);
    let fresh = plan_ps1_pbp_export(
        db,
        sources,
        &plan.set,
        &plan.destination,
        &plan.writer,
        cancel,
    )?;
    if fresh.receipt_path != plan.receipt_path
        || fresh.region != plan.region
        || fresh.dat_source != plan.dat_source
        || fresh.dat_revision != plan.dat_revision
        || fresh.dat_catalogue_sha256 != plan.dat_catalogue_sha256
    {
        return Err(refuse("export plan destinations or authority changed"));
    }
    if fresh
        .discs
        .iter()
        .map(|d| {
            (
                &d.cue,
                &d.bin,
                &d.cue_sha256,
                &d.bin_sha256,
                &d.serial,
                &d.dat_entry,
            )
        })
        .collect::<Vec<_>>()
        != plan
            .discs
            .iter()
            .map(|d| {
                (
                    &d.cue,
                    &d.bin,
                    &d.cue_sha256,
                    &d.bin_sha256,
                    &d.serial,
                    &d.dat_entry,
                )
            })
            .collect::<Vec<_>>()
    {
        return Err(refuse("export plan is stale"));
    }
    let parent = plan
        .destination
        .parent()
        .ok_or_else(|| refuse("destination has no parent"))?;
    let stage = tempfile::Builder::new()
        .prefix(".emuwiz-ps1-pbp-")
        .tempdir_in(parent)
        .map_err(|e| refuse(format!("private staging failed: {e}")))?;
    let stage_path = stage.path().to_path_buf();
    let input_dir = stage_path.join("input");
    let output_dir = stage_path.join("output");
    let extracted_dir = stage_path.join("extracted");
    fs::create_dir(&input_dir).map_err(|e| refuse(e.to_string()))?;
    fs::create_dir(&output_dir).map_err(|e| refuse(e.to_string()))?;
    fs::create_dir(&extracted_dir).map_err(|e| refuse(e.to_string()))?;
    let mut playlist = String::new();
    for disc in &plan.discs {
        progress(PbpExportProgress::Copying {
            disc: disc.ordinal,
            total: plan.discs.len(),
        });
        let folder = input_dir.join(format!("disc{}", disc.ordinal));
        fs::create_dir(&folder).map_err(|e| refuse(e.to_string()))?;
        let cue = folder.join(
            disc.cue
                .file_name()
                .ok_or_else(|| refuse("CUE has no name"))?,
        );
        let bin = folder.join(
            disc.bin
                .file_name()
                .ok_or_else(|| refuse("BIN has no name"))?,
        );
        fs::copy(&disc.cue, &cue).map_err(|e| refuse(e.to_string()))?;
        fs::copy(&disc.bin, &bin).map_err(|e| refuse(e.to_string()))?;
        if sha256(&cue, cancel)? != disc.cue_sha256 || sha256(&bin, cancel)? != disc.bin_sha256 {
            return Err(refuse("staged source copy differs from verified original"));
        }
        playlist.push_str(&format!("{}\n", cue.display()));
    }
    let playlist_path = stage_path.join("export.m3u");
    fs::write(&playlist_path, playlist).map_err(|e| refuse(e.to_string()))?;
    let output_flag = Path::new("-o");
    progress(PbpExportProgress::Packaging);
    run_writer(
        &plan.writer,
        &[
            Path::new("-i"),
            &playlist_path,
            output_flag,
            &output_dir,
            Path::new("-l"),
            Path::new("5"),
        ],
        &stage_path,
        cancel,
    )?;
    let staged_pbp = output_dir.join("export.pbp");
    if !fs::symlink_metadata(&staged_pbp).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
    {
        return Err(refuse("writer did not create exactly the expected PBP"));
    }
    if fs::read_dir(&output_dir)
        .map_err(|e| refuse(e.to_string()))?
        .count()
        != 1
    {
        return Err(refuse("writer created unexpected output files"));
    }
    progress(PbpExportProgress::Extracting);
    run_writer(
        &plan.writer,
        &[Path::new("-i"), &staged_pbp, output_flag, &extracted_dir],
        &stage_path,
        cancel,
    )?;
    let files = fs::read_dir(&extracted_dir)
        .map_err(|e| refuse(e.to_string()))?
        .map(|e| e.map(|item| item.path()).map_err(|e| refuse(e.to_string())))
        .collect::<Result<Vec<_>, _>>()?;
    if files.len() != plan.discs.len() * 2 {
        return Err(refuse("PBP extraction disc count is incomplete"));
    }
    let mut verified = Vec::new();
    for disc in &plan.discs {
        progress(PbpExportProgress::Comparing {
            disc: disc.ordinal,
            total: plan.discs.len(),
        });
        let cue = extracted_dir.join(format!("export - Disc {}.cue", disc.ordinal));
        let bin = extracted_dir.join(format!("export - Disc {}.bin", disc.ordinal));
        if !files.contains(&cue) || !files.contains(&bin) {
            return Err(refuse("PBP extraction order or count differs"));
        }
        let extracted = inspect_ps1_cue_disc(&cue).map_err(|e| refuse(e.to_string()))?;
        if extracted.track_count != 1
            || extracted.companions != vec![bin.clone()]
            || accepted_layout(&cue, &bin).is_err()
        {
            return Err(refuse(format!(
                "extracted disc {} track layout differs",
                disc.ordinal
            )));
        }
        let serial = inspect_game_identity(&cue, Some("PSX"))
            .verified_ps1_serial()
            .ok_or_else(|| refuse("extracted PS1 serial is not verified"))?
            .to_owned();
        let digest = sha256(&bin, cancel)?;
        if serial != disc.serial || digest != disc.bin_sha256 {
            return Err(refuse(format!(
                "extracted disc {} sector hash or serial differs",
                disc.ordinal
            )));
        }
        verified.push(PbpExportDiscReceipt {
            ordinal: disc.ordinal,
            cue: disc.cue.clone(),
            bin: disc.bin.clone(),
            cue_sha256: disc.cue_sha256.clone(),
            bin_sha256: disc.bin_sha256.clone(),
            serial,
            dat_entry: disc.dat_entry.clone(),
            roundtrip_bin_sha256: digest,
        });
    }
    recheck(plan, cancel)?;
    let final_authority = plan_ps1_pbp_export(
        db,
        sources,
        &plan.set,
        &plan.destination,
        &plan.writer,
        cancel,
    )?;
    if final_authority.dat_source != plan.dat_source
        || final_authority.dat_revision != plan.dat_revision
        || final_authority.dat_catalogue_sha256 != plan.dat_catalogue_sha256
        || final_authority
            .discs
            .iter()
            .map(|d| (&d.cue_sha256, &d.bin_sha256, &d.serial, &d.dat_entry))
            .collect::<Vec<_>>()
            != plan
                .discs
                .iter()
                .map(|d| (&d.cue_sha256, &d.bin_sha256, &d.serial, &d.dat_entry))
                .collect::<Vec<_>>()
    {
        return Err(refuse("DAT or source identity changed during export"));
    }
    File::open(&staged_pbp)
        .and_then(|file| file.sync_all())
        .map_err(|e| refuse(format!("cannot durably stage PBP: {e}")))?;
    let receipt = PbpExportReceipt {
        schema: "emuwiz.ps1-pbp-export.v1".into(),
        label: DERIVED_LABEL.into(),
        release: plan.set.identity.key.value.clone(),
        region: plan.region.clone(),
        dat_source: plan.dat_source.clone(),
        dat_revision: plan.dat_revision.clone(),
        dat_catalogue_sha256: plan.dat_catalogue_sha256.clone(),
        writer_release: WRITER_RELEASE.into(),
        writer_sha256: WRITER_SHA256.into(),
        pbp_sha256: sha256(&staged_pbp, cancel)?,
        discs: verified,
    };
    let staged_receipt = stage_path.join("export.receipt.json");
    let mut receipt_file = File::create(&staged_receipt).map_err(|e| refuse(e.to_string()))?;
    serde_json::to_writer_pretty(&mut receipt_file, &receipt).map_err(|e| refuse(e.to_string()))?;
    receipt_file
        .write_all(b"\n")
        .map_err(|e| refuse(e.to_string()))?;
    receipt_file.sync_all().map_err(|e| refuse(e.to_string()))?;
    // A new destination can appear during conversion. Refuse before journalling;
    // the repair engine repeats the no-clobber check at publication.
    if fs::symlink_metadata(&plan.destination).is_ok()
        || fs::symlink_metadata(&plan.receipt_path).is_ok()
    {
        return Err(refuse("destination appeared during export"));
    }
    let stage_name = stage_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| refuse("unsafe staging name"))?;
    let id = format!("ps1-pbp-{}-{stage_name}", now_unix());
    let repair_plan = build_repair_plan(
        RepairPlanId::new(id.clone()).ok_or_else(|| refuse("unsafe journal id"))?,
        0,
        now_unix(),
        Some("verified-ps1-pbp-export-v1".into()),
        // Repair sorts by proposal id. Put the receipt first so an interrupted
        // transaction cannot expose a PBP without its verification evidence.
        vec![
            proposal(
                format!("{id}-a-receipt"),
                staged_receipt,
                plan.receipt_path.clone(),
            )?,
            proposal(format!("{id}-z-pbp"), staged_pbp, plan.destination.clone())?,
        ],
    );
    let mut tx = build_repair_transaction(&repair_plan).map_err(|e| refuse(e.to_string()))?;
    let retained = stage.keep();
    let options = RepairExecutionOptions {
        trusted: TrustedRoots::from_paths([retained.as_path(), parent]),
        journal_dir: journal_dir.to_path_buf(),
        audit_cache: crate::dat::sources::audit_cache::AuditCacheConfig::Disabled,
    };
    progress(PbpExportProgress::Publishing);
    let outcome = apply_repair_transaction(&mut RepairApplyExecution {
        transaction: &mut tx,
        current_generation: 0,
        options: &options,
        cancel,
    });
    let confirmed = outcome.as_ref().is_ok_and(|result| {
        result.transaction.state == TransactionState::Applied
            && result.summary.applied == 2
            && result.summary.failed == 0
            && result.reverify.len() == 2
            && result
                .reverify
                .iter()
                .all(|entry| entry.outcome == RepairReverifyOutcome::Verified)
    });
    if !confirmed {
        progress(PbpExportProgress::Recovering);
        let detail = outcome
            .err()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "transaction did not confirm both outputs".into());
        let recovery = if tx.state == TransactionState::Planned {
            PbpRecoveryStatus::NothingPublished {
                transaction_id: tx.transaction_id.clone(),
                stage: retained.clone(),
            }
        } else {
            match rollback_repair_transaction(&mut tx, journal_dir, &AtomicBool::new(false)) {
                Ok(crate::dat::rename_apply::model::RollbackResult::FullyRolledBack) => {
                    PbpRecoveryStatus::RolledBack {
                        transaction_id: tx.transaction_id.clone(),
                        stage: retained.clone(),
                    }
                }
                Ok(other) => PbpRecoveryStatus::NeedsReview {
                    transaction_id: tx.transaction_id.clone(),
                    stage: retained.clone(),
                    detail: format!("rollback incomplete ({other:?})"),
                },
                Err(error) => PbpRecoveryStatus::NeedsReview {
                    transaction_id: tx.transaction_id.clone(),
                    stage: retained.clone(),
                    detail: format!("rollback failed ({error})"),
                },
            }
        };
        return Err(PbpExportError {
            detail: format!("publication not confirmed: {detail}; {recovery:?}"),
            recovery: Some(recovery),
        });
    }
    let result = outcome.expect("confirmed outcome was Ok");
    let mut cleanup_errors = Vec::new();
    for path in [stage_path.join("input"), extracted_dir] {
        if let Err(error) = fs::remove_dir_all(&path) {
            cleanup_errors.push(format!("{}: {error}", path.display()));
        }
    }
    for path in [playlist_path, stage_path.join("writer.log")] {
        if let Err(error) = fs::remove_file(&path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            cleanup_errors.push(format!("{}: {error}", path.display()));
        }
    }
    Ok(PbpExportResult {
        receipt,
        transaction: result,
        recovery_stage: retained,
        cleanup_warning: (!cleanup_errors.is_empty()).then(|| cleanup_errors.join("; ")),
    })
}

#[cfg(test)]
#[path = "ps1_pbp_export/tests.rs"]
mod tests;
