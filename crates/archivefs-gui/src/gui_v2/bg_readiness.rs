//! Bounded, cancellable background launch-readiness checks for the games on
//! screen in Blue Browse & Play.
//!
//! The worker never touches the selected game, the selected-evidence slot,
//! the details panel, a launch handle or any shared emulator controller. It
//! builds its own evidence and its own emulator-state snapshot, reuses the
//! same input builder and projection the selected game uses, and returns
//! either a finished result or nothing. Nothing is a result unless it is
//! known: whatever the worker cannot determine stays Not checked.
//!
//! TEMPORARY LIMIT: some platforms take part of their answer from controllers
//! that are only populated when the game is selected. The worker skips them
//! (see [`selection_bound_platform`]); they stay Not checked until the game is
//! selected. This is not full background coverage.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

use archivefs_core::PersistedArchive;

use super::library::{CheckedReadiness, Game};
use crate::*;

/// Checks running at once. Small on purpose: each reads the game's headers.
pub(super) const MAX_IN_FLIGHT: usize = 2;
/// Games considered per frame, taken from the ones drawn.
pub(super) const MAX_CANDIDATES: usize = 24;

/// Canonical platform ids whose readiness depends on selected-game-only
/// controller state (Flycast GDI, VICE, XRoar cassette, Cemu, plus the
/// Hatari, Mesen and shadPS4 snapshots, which are the same kind). Compared
/// case-insensitively against the catalogue, the evidence report and the plan.
const SELECTION_BOUND_PLATFORMS: [&str; 9] = [
    "Dreamcast",
    "Commodore 64",
    archivefs_core::launch::DRAGON_COCO_PLATFORM_ID,
    "WiiU",
    "Wii U",
    "AtariST",
    "Atari ST",
    "NES",
    "PS4",
];

pub(super) fn selection_bound_platform(platform: &str) -> bool {
    let canonical = archivefs_core::canonical_platform_for_alias(platform).unwrap_or(platform);
    SELECTION_BOUND_PLATFORMS
        .iter()
        .any(|bound| bound.eq_ignore_ascii_case(canonical) || bound.eq_ignore_ascii_case(platform))
}

/// Immutable copy of the emulator readiness the builder reads. Only finished
/// scans are copied; anything still scanning is `None`, and the worker waits.
#[derive(Clone)]
pub(super) struct EnvSnapshot {
    retroarch: Option<RetroArchCheatSetupDiscovery>,
    dolphin: Option<(
        archivefs_core::patch_manager::DolphinLocalProfileDiscovery,
        archivefs_core::patch_manager::DolphinLocalDiscoveryRoots,
    )>,
    pcsx2: Option<(
        archivefs_core::patch_manager::Pcsx2ProfileDiscovery,
        archivefs_core::patch_manager::Pcsx2ProfileDiscoveryRoots,
    )>,
    firmware: Option<Vec<archivefs_core::dat::firmware_evidence::FirmwareIdentityRecord>>,
    xenia: XeniaProfilesState,
    overrides: crate::emulator_setup_overrides::EmulatorPathOverrides,
}

impl EnvSnapshot {
    /// A cheap fingerprint of everything [`Self::capture`] reads, or `None`
    /// while a needed scan is unfinished. A different value means earlier
    /// background answers no longer describe the machine.
    pub(super) fn signature(state: &EmulatorReadinessState) -> Option<u64> {
        use std::hash::{Hash, Hasher};
        let RetroArchProfilesState::Ready(retroarch) = &state.retroarch_profiles else {
            return None;
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        format!("{:?}", retroarch.environment).hash(&mut hasher);
        format!("{:?}", state.retroarch_installation_fingerprint).hash(&mut hasher);
        format!("{:?}", state.retroarch_core_directory_override).hash(&mut hasher);
        format!("{:?}", state.emulator_setup_overrides).hash(&mut hasher);
        format!("{:?}", state.xenia_profiles).hash(&mut hasher);
        match &state.dolphin_local_profiles {
            DolphinLocalProfilesState::Ready(r) => format!("{:?}", r.discovery).hash(&mut hasher),
            DolphinLocalProfilesState::Error(_) => 1u8.hash(&mut hasher),
            _ => return None,
        }
        match &state.pcsx2_launch_profiles {
            Pcsx2LaunchProfilesState::Ready(r) => format!("{:?}", r.discovery).hash(&mut hasher),
            Pcsx2LaunchProfilesState::Error(_) => 1u8.hash(&mut hasher),
            _ => return None,
        }
        match &state.pcsx2_firmware_evidence {
            Pcsx2FirmwareEvidenceState::Ready(records) => records.len().hash(&mut hasher),
            Pcsx2FirmwareEvidenceState::Error(_) => 1u8.hash(&mut hasher),
            _ => return None,
        }
        Some(hasher.finish())
    }

    /// `None` until the scans every launch decision needs have finished.
    pub(super) fn capture(state: &EmulatorReadinessState) -> Option<Self> {
        let RetroArchProfilesState::Ready(discovery) = &state.retroarch_profiles else {
            return None;
        };
        let dolphin = match &state.dolphin_local_profiles {
            DolphinLocalProfilesState::Ready(ready) => {
                Some((ready.discovery.clone(), ready.roots.clone()))
            }
            DolphinLocalProfilesState::Error(_) => None,
            _ => return None,
        };
        let pcsx2 = match &state.pcsx2_launch_profiles {
            Pcsx2LaunchProfilesState::Ready(ready) => {
                Some((ready.discovery.clone(), ready.roots.clone()))
            }
            Pcsx2LaunchProfilesState::Error(_) => None,
            _ => return None,
        };
        let firmware = match &state.pcsx2_firmware_evidence {
            Pcsx2FirmwareEvidenceState::Ready(records) => Some(records.clone()),
            Pcsx2FirmwareEvidenceState::Error(_) => None,
            _ => return None,
        };
        Some(Self {
            retroarch: Some(discovery.clone()),
            dolphin,
            pcsx2,
            firmware,
            xenia: state.xenia_profiles.clone(),
            overrides: state.emulator_setup_overrides.clone(),
        })
    }

    /// A fresh readiness state holding exactly this snapshot. Selection-bound
    /// controllers stay empty and are never shared with the app.
    fn to_state(&self) -> EmulatorReadinessState {
        let mut state = EmulatorReadinessState::default();
        // Rebuilt from clones only; the discovery wrapper is the app's own type.
        if let Some(discovery) = &self.retroarch {
            state.retroarch_profiles = RetroArchProfilesState::Ready(discovery.clone());
        }
        if let Some((discovery, roots)) = &self.dolphin {
            state.dolphin_local_profiles =
                DolphinLocalProfilesState::Ready(DolphinLocalProfilesReady {
                    discovery: discovery.clone(),
                    roots: roots.clone(),
                });
        }
        if let Some((discovery, roots)) = &self.pcsx2 {
            state.pcsx2_launch_profiles =
                Pcsx2LaunchProfilesState::Ready(Pcsx2LaunchProfilesReady {
                    discovery: discovery.clone(),
                    roots: roots.clone(),
                });
        }
        state.pcsx2_firmware_evidence =
            Pcsx2FirmwareEvidenceState::Ready(self.firmware.clone().unwrap_or_default());
        state.xenia_profiles = self.xenia.clone();
        state.emulator_setup_overrides = self.overrides.clone();
        state
    }
}

/// One game to check, with everything the worker needs and nothing shared.
#[derive(Clone)]
pub(super) struct Job {
    pub(super) id: i64,
    pub(super) archive: PersistedArchive,
    pub(super) catalogue_platform: String,
    pub(super) dat_exact: bool,
}

impl Job {
    pub(super) fn for_game(game: &Game) -> Self {
        Self {
            id: game.archive.id,
            archive: game.archive.clone(),
            catalogue_platform: game.platform.clone(),
            dat_exact: game.dat_exact.is_some(),
        }
    }
}

/// What a worker run concluded. `Unknown` is never recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Known(CheckedReadiness),
    /// The platform needs selected-game-only evidence; left Not checked.
    Skipped,
    /// No honest answer (checking, partial or unresolved evidence, error).
    Unknown,
}

/// Run one check. Pure with respect to the app: reads the file's headers and
/// the snapshot, writes nothing, launches nothing.
pub(super) fn check(env: &EnvSnapshot, job: &Job, cancel: &AtomicBool) -> Outcome {
    use super::launch_readiness_summary as summary;
    if selection_bound_platform(&job.catalogue_platform) {
        return Outcome::Skipped;
    }
    let path = job.archive.absolute_path.clone();
    let hint = job.archive.platform.as_deref();
    let Ok(report) = crate::selected_evidence_page::gather_selected_evidence_fast(&path, hint)
    else {
        return Outcome::Unknown;
    };
    if cancel.load(Ordering::Relaxed) {
        return Outcome::Unknown;
    }
    if selection_bound_platform(&format!("{:?}", report.game_identity_report.platform)) {
        return Outcome::Skipped;
    }
    let state = env.to_state();
    let evidence = crate::selected_evidence_page::SelectedEvidenceState::Ready {
        generation: 0,
        report: Box::new(report),
        hasheous: Default::default(),
    };
    let archive = &job.archive;
    let scummvm = archive.archive_kind == "scummvm_directory";
    let freshness_for = |_: &Path, _: Option<&LoadedData>| {
        archivefs_core::launch::EvidenceFreshness::from_stored_health(&archive.last_known_health)
    };
    let member_path = |_: &ArchiveRecord| None;
    let is_scummvm_folder = |p: &Path| scummvm && p == archive.absolute_path;
    let loose_ref = |p: &Path| {
        let kind =
            archivefs_core::ArchiveKind::from_storage(&archive.archive_kind).filter(|kind| {
                !kind.is_mount_input()
                    && *kind != archivefs_core::ArchiveKind::ScummVmDirectory
                    && archive.last_verified_missing_at.is_none()
            })?;
        if p != archive.absolute_path || !std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file()) {
            return None;
        }
        archivefs_core::launch::launch_content_ref_for_unmounted(kind, p)
    };
    let cheat_workflow = None;
    let input = crate::emulator_setup::readiness_input::ReadinessSources {
        readiness: &state,
        evidence: &evidence,
        focused: Some(&path),
        cheat_workflow: &cheat_workflow,
        database_snapshot: None,
        database_generation: 0,
        freshness_for: &freshness_for,
        member_path: &member_path,
        is_scummvm_folder: &is_scummvm_folder,
        loose_ref: &loose_ref,
    }
    .build(None);
    // Only a resolved plan is an answer. An unresolved identity may be one a
    // selection-bound controller would have supplied, so it is not a verdict.
    let crate::launch_readiness_page::LaunchReadinessInput::Plan {
        plan, retroarch, ..
    } = &input
    else {
        return Outcome::Unknown;
    };
    if plan
        .platform_id
        .as_deref()
        .is_some_and(selection_bound_platform)
    {
        return Outcome::Skipped;
    }
    let mut projected = summary::project(
        &input,
        super::launch_readiness_summary::ReadinessFreshness::Current,
    );
    if let Some(candidate) = summary::selected_candidate(plan)
        && matches!(
            candidate.target,
            archivefs_core::launch::LaunchTarget::RetroArchCore { .. }
        )
        && let Err(refusal) = crate::launch_readiness_page::retroarch_launch_request_checked(
            plan,
            candidate,
            retroarch.as_ref(),
        )
    {
        summary::note_retroarch_installation_refusal(&mut projected, &refusal);
    }
    if job.dat_exact {
        summary::note_catalogue_verified(&mut projected);
    }
    // Whole-file checksum pending means identity is not final: not a verdict.
    let pending_checksum = match &evidence {
        crate::selected_evidence_page::SelectedEvidenceState::Ready { report, .. } => {
            crate::selected_evidence_page::needs_selected_evidence_enrichment(report)
        }
        _ => false,
    };
    match CheckedReadiness::from_compact(&summary::compact(&projected, pending_checksum)) {
        Some(result) => Outcome::Known(result),
        None => Outcome::Unknown,
    }
}

struct Finished {
    epoch: u64,
    id: i64,
    outcome: Outcome,
}

/// Schedules checks for the games drawn, a few at a time. Owned by the
/// browse state; dropping or invalidating it cancels outstanding work.
pub(super) struct Scheduler {
    epoch: u64,
    cancel: Arc<AtomicBool>,
    in_flight: usize,
    /// Games already decided this epoch (checked, skipped or unknown), so a
    /// game is not retried on every frame.
    settled: HashMap<i64, Outcome>,
    sender: Sender<Finished>,
    receiver: Receiver<Finished>,
}

impl Default for Scheduler {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            epoch: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            in_flight: 0,
            settled: HashMap::new(),
            sender,
            receiver,
        }
    }
}

impl Scheduler {
    /// Emulator settings, scans or the catalogue changed: stop work, forget
    /// what was decided, and make every outstanding result stale.
    pub(super) fn invalidate(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.epoch += 1;
        self.settled.clear();
        // Results from the old epoch still arrive and are dropped in `drain`.
    }

    pub(super) fn epoch(&self) -> u64 {
        self.epoch
    }

    pub(super) fn in_flight(&self) -> usize {
        self.in_flight
    }

    /// Collect finished checks. Returns only current-epoch outcomes.
    pub(super) fn drain(&mut self) -> Vec<(i64, Outcome)> {
        let mut current = Vec::new();
        while let Ok(done) = self.receiver.try_recv() {
            self.in_flight = self.in_flight.saturating_sub(1);
            if done.epoch == self.epoch {
                self.settled.insert(done.id, done.outcome);
                current.push((done.id, done.outcome));
            }
        }
        current
    }

    /// Games that still need a check, nearest first, within capacity.
    pub(super) fn pick<'a>(
        &self,
        visible: impl Iterator<Item = &'a Game>,
        is_cached: impl Fn(&Game) -> bool,
    ) -> Vec<Job> {
        let room = MAX_IN_FLIGHT.saturating_sub(self.in_flight);
        visible
            .take(MAX_CANDIDATES)
            .filter(|game| {
                !game.is_preservation_only() && !self.settled.contains_key(&game.archive.id)
            })
            .filter(|game| !is_cached(game))
            .take(room)
            .map(Job::for_game)
            .collect()
    }

    pub(super) fn spawn(&mut self, env: &EnvSnapshot, job: Job, context: egui::Context) {
        self.in_flight += 1;
        // Mark it so a slow check is not queued twice.
        self.settled.insert(job.id, Outcome::Unknown);
        let (epoch, cancel, sender) = (self.epoch, Arc::clone(&self.cancel), self.sender.clone());
        let env = env.clone();
        std::thread::spawn(move || {
            let outcome = if cancel.load(Ordering::Relaxed) {
                Outcome::Unknown
            } else {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    check(&env, &job, &cancel)
                }))
                .unwrap_or(Outcome::Unknown)
            };
            let _ = sender.send(Finished {
                epoch,
                id: job.id,
                outcome,
            });
            context.request_repaint();
        });
    }
}
