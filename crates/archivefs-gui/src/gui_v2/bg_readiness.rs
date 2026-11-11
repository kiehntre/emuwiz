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

/// Platforms skipped for now, each with the selection-bound controller that
/// makes a background answer unsafe. That controller contributes a launch
/// candidate (or the identity itself) only after the game is selected, and the
/// readiness plan is one result over all candidates, so a background plan
/// without it could say Not ready, or Ready where the selected game would be
/// asked to choose. The whole platform is therefore skipped, not just that
/// emulator. TEMPORARY: the follow-up supplies selection-independent evidence
/// and removes entries one at a time, NES/Mesen first. What each still needs:
/// Dreamcast (Flycast GDI): `flycast_profiles` in the snapshot plus a bounded,
/// cancel-aware disc read (CHD/CDI have no path-only evidence at all); C64
/// (VICE), Wii U (Cemu), Atari ST (Hatari), NES (Mesen), PS4 (shadPS4): their
/// own environment data in the snapshot. Dragon/CoCo cassette is NOT skipped:
/// its check needs only the file path, so the worker runs it itself.
const SELECTION_BOUND_PLATFORMS: [(&str, &str); 8] = [
    ("Dreamcast", "Flycast GDI context"),
    ("Commodore 64", "VICE context"),
    ("WiiU", "Cemu context and inspection"),
    ("Wii U", "Cemu context and inspection"),
    ("AtariST", "Hatari snapshot"),
    ("Atari ST", "Hatari snapshot"),
    ("NES", "Mesen snapshot"),
    ("PS4", "shadPS4 snapshot"),
];

pub(super) fn selection_bound_platform(platform: &str) -> bool {
    let canonical = archivefs_core::canonical_platform_for_alias(platform).unwrap_or(platform);
    SELECTION_BOUND_PLATFORMS.iter().any(|(bound, _)| {
        bound.eq_ignore_ascii_case(canonical) || bound.eq_ignore_ascii_case(platform)
    })
}

/// The same skip rule, from the evidence report's typed platform (the typed
/// ids, not their Debug text).
pub(super) fn selection_bound_identity(
    platform: archivefs_core::game_identity::IdentityPlatform,
) -> bool {
    use archivefs_core::game_identity::IdentityPlatform as P;
    matches!(
        platform,
        P::Dreamcast | P::Commodore64 | P::WiiU | P::AtariST | P::Nes | P::PlayStation4
    )
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
    /// The user's remembered emulator choice decides between several safe
    /// candidates, so the plan must see the same memory the foreground does.
    remembered: Vec<RememberedEmulatorProfile>,
    /// Explicit launch choices decide ties the same way in the background.
    launch_choices: crate::launch_readiness_page::launch_choice::LaunchChoiceState,
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
        format!("{:?}", state.remembered_emulator_profiles).hash(&mut hasher);
        // Only what changes a plan; the launch-observation flag does not.
        format!(
            "{:?}{:?}",
            state.launch_choices.preferences, state.launch_choices.once
        )
        .hash(&mut hasher);
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
            remembered: state.remembered_emulator_profiles.clone(),
            launch_choices: state.launch_choices.clone(),
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
        state.remembered_emulator_profiles = self.remembered.clone();
        state.launch_choices = self.launch_choices.clone();
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
    /// The game belongs to a multi-disc set. The foreground plan uses the
    /// library snapshot for that; a background check has none, so it must not
    /// answer (a disc set may need review where this would say Ready).
    pub(super) multidisc: bool,
}

impl Job {
    pub(super) fn for_game(game: &Game) -> Self {
        Self {
            id: game.archive.id,
            archive: game.archive.clone(),
            catalogue_platform: game.platform.clone(),
            dat_exact: game.dat_exact.is_some(),
            multidisc: false,
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
    if job.multidisc || selection_bound_platform(&job.catalogue_platform) {
        return Outcome::Skipped;
    }
    let (input, pending_checksum) = match build_input(env, job, cancel) {
        Ok(built) => built,
        Err(outcome) => return outcome,
    };
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
    if matches!(
        projected.status,
        summary::ReadinessPresentationState::Ready
            | summary::ReadinessPresentationState::ReadyWithWarnings
    ) {
        summary::route_truth::apply(&mut projected, &summary::route_truth::assess(&input));
    }
    if job.dat_exact {
        summary::note_catalogue_verified(&mut projected);
    }
    match CheckedReadiness::from_compact(&summary::compact(&projected, pending_checksum)) {
        Some(result) => Outcome::Known(result),
        None => Outcome::Unknown,
    }
}

/// The readiness input a background check plans from: the same sources and
/// builder the selected game uses, from this job's file and the snapshot.
/// `Err` carries the outcome when nothing honest can be said (unreadable or
/// cancelled: Unknown; a selection-bound identity: Skipped). The flag is whether the whole-file checksum is
/// still pending.
pub(super) fn build_input(
    env: &EnvSnapshot,
    job: &Job,
    cancel: &AtomicBool,
) -> Result<(crate::launch_readiness_page::LaunchReadinessInput, bool), Outcome> {
    let path = job.archive.absolute_path.clone();
    let hint = job.archive.platform.as_deref();
    let Ok(report) = crate::selected_evidence_page::gather_selected_evidence_fast(&path, hint)
    else {
        return Err(Outcome::Unknown);
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(Outcome::Unknown);
    }
    if selection_bound_identity(report.game_identity_report.platform) {
        return Err(Outcome::Skipped);
    }
    let mut state = env.to_state();
    // Dragon/CoCo cassette evidence needs only the file: run the same bounded
    // path-only check the selected game runs (one small file, hashed once).
    if crate::emulator_setup::xroar_cassette::is_cas(&path)
        && archivefs_core::canonical_platform_for_alias(&job.catalogue_platform)
            .unwrap_or(&job.catalogue_platform)
            .eq_ignore_ascii_case(archivefs_core::launch::DRAGON_COCO_PLATFORM_ID)
    {
        if cancel.load(Ordering::Relaxed) {
            return Err(Outcome::Unknown);
        }
        let context = crate::launch_readiness_page::xroar::Context::inspect(path.clone());
        state.xroar_cassette =
            crate::emulator_setup::xroar_cassette::Controller::with_context(path.clone(), context);
    }
    // Loose .3dsx homebrew needs only the file: the same bounded check the
    // selected game runs (one small file, hashed once). A catalogue platform
    // that is not Nintendo 3DS means the folder says otherwise: skip.
    if crate::launch_readiness_page::azahar::is_three_dsx(&path)
        && crate::launch_readiness_page::azahar::platform_allows(Some(&job.catalogue_platform))
    {
        if cancel.load(Ordering::Relaxed) {
            return Err(Outcome::Unknown);
        }
        let context = crate::launch_readiness_page::azahar::Context::inspect(path.clone());
        state.azahar_homebrew =
            crate::emulator_setup::azahar_homebrew::Controller::with_context(path.clone(), context);
    }
    // A Vita .vpk: the same bounded check the selected game runs (ZIP directory
    // and one param.sfo member, plus a Vita3K rescan).
    if archivefs_core::launch::is_vpk(&path)
        && crate::launch_readiness_page::vita3k::platform_allows(Some(&job.catalogue_platform))
    {
        if cancel.load(Ordering::Relaxed) {
            return Err(Outcome::Unknown);
        }
        let context = crate::launch_readiness_page::vita3k::Context::inspect(path.clone());
        state.vita3k_library =
            crate::emulator_setup::vita3k_library::Controller::with_context(path.clone(), context);
    }
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
        if p != archive.absolute_path
            || !crate::emulator_setup::catalogued_content_shape_ok(kind, p)
        {
            return None;
        }
        archivefs_core::launch::launch_content_ref_for_unmounted(kind, p)
    };
    let platform_hint = |p: &Path| {
        (p == archive.absolute_path)
            .then(|| archive.platform.clone())
            .flatten()
    };
    let platform_source = |p: &Path| {
        (p == archive.absolute_path)
            .then(|| archive.platform_source.clone())
            .flatten()
    };
    let cheat_workflow = None;
    let is_arcade_set_directory =
        |p: &Path| archive.archive_kind == "arcade_set_directory" && p == archive.absolute_path;
    // This worker is off the UI thread, so it may wait for the (cached,
    // de-duplicated, capped) MAME probe; cancelling the batch cancels the wait.
    let input = crate::arcade_mame_probe::with_blocking(cancel, || {
        crate::emulator_setup::readiness_input::ReadinessSources {
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
            is_arcade_set_directory: &is_arcade_set_directory,
            platform_hint: &platform_hint,
            platform_source: &platform_source,
            system_trial: &|_| None,
            // A saved "Remember this launch choice" is the person's lasting
            // answer for this unchanged file, so the list shows it too.
            remembered_system:
                &archivefs_core::launch::remembered_launch_system::remembered_launch_system,
            system_unknown: &|p| p == archive.absolute_path && archive.platform.is_none(),
        }
        .build(None)
    });
    // Whole-file checksum pending means identity is not final: not a verdict.
    let pending_checksum = match &evidence {
        crate::selected_evidence_page::SelectedEvidenceState::Ready { report, .. } => {
            crate::selected_evidence_page::needs_selected_evidence_enrichment(report)
        }
        _ => false,
    };
    Ok((input, pending_checksum))
}

/// Whether the foreground checker would plan `path` with multi-disc set
/// context (the same lookup it uses). Background checks have none.
pub(super) fn in_disc_set(
    sets: &[archivefs_core::media_set::MediaSet],
    path: &Path,
    archives: &[PersistedArchive],
) -> bool {
    crate::multidisc_launch_view::MultiDiscLaunchContext::for_content(sets, path, archives)
        .is_some()
}

/// Apply one finished check to the cache. Only a known answer is kept, and
/// only where no current answer exists; skipped and unknown outcomes leave the
/// game Not checked.
pub(super) fn record_outcome(
    cache: &mut super::library::ReadinessCache,
    game: &Game,
    outcome: Outcome,
) {
    if let Outcome::Known(result) = outcome {
        cache.record_background(game, result);
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

    #[cfg(test)]
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

    /// Mark a game decided without running anything (it is not eligible yet).
    pub(super) fn settle(&mut self, id: i64) {
        self.settled.insert(id, Outcome::Skipped);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui_v2::library::{CheckedReadiness as C, ReadinessCache};

    fn archive(id: i64, platform: &str) -> PersistedArchive {
        PersistedArchive {
            id,
            source_folder_id: 1,
            relative_path: format!("{id}.bin").into(),
            absolute_path: format!("/nonexistent-bg-readiness/{id}.bin").into(),
            archive_kind: "file".into(),
            display_name: format!("Game {id}"),
            normalized_name: format!("game {id}"),
            size_bytes: Some(1),
            modified_time_unix_seconds: Some(1),
            platform: Some(platform.into()),
            platform_source: Some("fixture".into()),
            last_known_health: "ok".into(),
            last_seen_at: "fixture".into(),
            last_verified_missing_at: None,
            identity_report: None,
        }
    }

    fn game(id: i64, platform: &str) -> Game {
        Game::from_archive(archive(id, platform))
    }

    fn env() -> EnvSnapshot {
        EnvSnapshot {
            retroarch: None,
            dolphin: None,
            pcsx2: None,
            firmware: None,
            xenia: XeniaProfilesState::NotScanned,
            overrides: Default::default(),
            remembered: Vec::new(),
            launch_choices: Default::default(),
        }
    }

    fn on_disk(dir: &tempfile::TempDir, name: &str, platform: &str, bytes: &[u8]) -> Job {
        on_disk_from(dir, name, platform, Some("folder_alias"), bytes)
    }

    fn on_disk_from(
        dir: &tempfile::TempDir,
        name: &str,
        platform: &str,
        source: Option<&str>,
        bytes: &[u8],
    ) -> Job {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        let mut archive = archive(41, platform);
        archive.platform_source = source.map(str::to_string);
        archive.absolute_path = path.clone();
        archive.relative_path = name.into();
        archive.archive_kind = "direct_game_image".into();
        Job::for_game(&Game::from_archive(archive))
    }

    fn planned(job: &Job) -> Option<archivefs_core::launch::LaunchPlan> {
        match build_input(&env(), job, &AtomicBool::new(false)) {
            Ok((crate::launch_readiness_page::LaunchReadinessInput::Plan { plan, .. }, _)) => {
                Some(plan)
            }
            _ => None,
        }
    }

    /// Owner evidence 2026-11-09 (PC Engine CD, CD-i, Dreamcast, ScummVM,
    /// SNES): a game whose system is known but which has no header serial or
    /// catalogue match must reach a launch plan, never the identity loop. Its
    /// identity stays explicitly unverified.
    #[test]
    fn a_game_of_a_known_system_without_catalogue_identity_reaches_a_plan() {
        let dir = tempfile::tempdir().unwrap();
        // A CD-i disc image: no header serial, no loose-ROM hash identity.
        // Owner policy 2026-11-09: the folder alone is not enough for a shared
        // disc format - the background check reports it, never plans it.
        let name = "7th Guest The (USA) (Disc 1).chd";
        let folder = on_disk(&dir, name, "Philips CD-i", &[0u8; 512]);
        assert!(matches!(
            build_input(&env(), &folder, &AtomicBool::new(false)),
            Ok((
                crate::launch_readiness_page::LaunchReadinessInput::IdentityUnknown(
                    crate::launch_readiness_page::IdentityUnknownReason::FolderOnly("Philips CD-i")
                ),
                _
            ))
        ));
        // The person's own saved system choice reaches a plan.
        let job = on_disk_from(
            &dir,
            name,
            "Philips CD-i",
            Some(archivefs_core::MANUAL_PLATFORM_SOURCE),
            &[0u8; 512],
        );
        let plan = planned(&job).expect("a launch plan, not Identity unresolved");
        assert_eq!(plan.platform_id.as_deref(), Some("Philips CD-i"));
        let key = plan.game_key.clone().unwrap_or_default();
        assert!(
            archivefs_core::launch::unverified_media::is_unverified_key(&key),
            "key {key:?}"
        );
    }

    /// Owner decision 2026-11-10: a ticked "Remember this launch choice"
    /// saves the system for this unchanged file, so a folder-only game is not
    /// confirmed again on every play. It never becomes verified identity and
    /// stops applying when forgotten or when the file changes.
    #[test]
    fn a_remembered_launch_choice_plans_a_folder_only_game_until_forgotten_or_changed() {
        use archivefs_core::launch::remembered_launch_system as remembered;
        let dir = tempfile::tempdir().unwrap();
        let name = "7th Guest The (USA) (Disc 1).chd";
        let job = on_disk(&dir, name, "Philips CD-i", &[0u8; 512]);
        let path = job.archive.absolute_path.clone();
        let folder_only = |job: &Job| {
            matches!(
                build_input(&env(), job, &AtomicBool::new(false)),
                Ok((
                    crate::launch_readiness_page::LaunchReadinessInput::IdentityUnknown(
                        crate::launch_readiness_page::IdentityUnknownReason::FolderOnly(_)
                    ),
                    _
                ))
            )
        };
        assert!(folder_only(&job));

        let store = remembered::default_remembered_launch_systems_path().unwrap();
        let entry =
            remembered::RememberedLaunchSystem::for_current_file(&path, "Philips CD-i").unwrap();
        remembered::remember_launch_system_at(&store, &entry).unwrap();
        let plan = planned(&job).expect("a remembered choice reaches a launch plan");
        assert_eq!(plan.platform_id.as_deref(), Some("Philips CD-i"));
        let key = plan.game_key.clone().unwrap_or_default();
        assert!(
            archivefs_core::launch::unverified_media::is_unverified_key(&key),
            "never a verified identity: {key:?}"
        );

        // A changed file is not the file that was chosen.
        std::fs::write(&path, [1u8; 600]).unwrap();
        assert!(folder_only(&job));
        // Forgetting restores the question.
        let entry =
            remembered::RememberedLaunchSystem::for_current_file(&path, "Philips CD-i").unwrap();
        remembered::remember_launch_system_at(&store, &entry).unwrap();
        assert!(planned(&job).is_some());
        remembered::forget_launch_system_at(&store, &path).unwrap();
        assert!(folder_only(&job));
    }

    /// Owner decision 2026-11-10 (1,815 games with no system): Blue said
    /// "Identity unresolved / Verify identity" because it had no live record
    /// to show the system was missing. It now says no system is chosen and,
    /// with a remembered launch choice, plans as that system - unverified.
    #[test]
    fn a_game_with_no_recorded_system_says_so_and_a_remembered_choice_plans_it() {
        use archivefs_core::launch::remembered_launch_system as remembered;
        let dir = tempfile::tempdir().unwrap();
        let mut job = on_disk_from(
            &dir,
            "Avenger (Japan).chd",
            "PC Engine CD",
            None,
            &[0u8; 512],
        );
        job.archive.platform = None;
        job.catalogue_platform = String::new();
        // No RetroArch in the test environment: no system can be offered for
        // a try, so the truthful reason is "no system chosen".
        assert!(matches!(
            build_input(&env(), &job, &AtomicBool::new(false)),
            Ok((
                crate::launch_readiness_page::LaunchReadinessInput::IdentityUnknown(
                    crate::launch_readiness_page::IdentityUnknownReason::SystemNotChosen
                ),
                _
            ))
        ));
        let path = job.archive.absolute_path.clone();
        let store = remembered::default_remembered_launch_systems_path().unwrap();
        let entry =
            remembered::RememberedLaunchSystem::for_current_file(&path, "PC Engine CD").unwrap();
        remembered::remember_launch_system_at(&store, &entry).unwrap();
        let plan = planned(&job).expect("a remembered choice reaches a launch plan");
        assert_eq!(plan.platform_id.as_deref(), Some("PC Engine CD"));
        assert!(archivefs_core::launch::unverified_media::is_unverified_key(
            plan.game_key.as_deref().unwrap_or_default()
        ));
        // The library row is untouched: still no system.
        assert!(job.archive.platform.is_none());
        remembered::forget_launch_system_at(&store, &path).unwrap();
    }

    /// Owner regression list 2026-11-09: NES games misfiled under SNES.
    #[test]
    fn an_nes_game_renamed_into_the_snes_folder_never_plans_as_snes() {
        let dir = tempfile::tempdir().unwrap();
        let mut ines = b"NES\x1a".to_vec();
        ines.resize(16 + 32 * 1024, 0);
        for (name, source) in [
            ("Misfiled (USA).sfc", Some("folder_alias")),
            (
                "Misfiled (USA).sfc",
                Some(archivefs_core::MANUAL_PLATFORM_SOURCE),
            ),
            ("Misfiled (USA).bin", Some("folder_alias")),
        ] {
            let job = on_disk_from(&dir, name, "SNES", source, &ines);
            let input = build_input(&env(), &job, &AtomicBool::new(false));
            let as_snes = matches!(
                &input,
                Ok((crate::launch_readiness_page::LaunchReadinessInput::Plan { plan, .. }, _))
                    if plan.platform_id.as_deref() == Some("SNES")
            );
            assert!(!as_snes, "{name} {source:?} planned as SNES");
        }
    }

    /// Owner rule 7 (2026-11-09): formats EmuWiz explicitly does not start
    /// from here stay unsupported even with a saved system choice. IPF is
    /// still one; a hard-disk image no longer is (2026-11-11: it starts from
    /// a private working copy), so it must not be refused as one.
    #[test]
    fn amiga_ipf_stays_explicitly_unsupported_and_hard_disk_images_do_not() {
        use crate::launch_readiness_page::{IdentityUnknownReason as R, LaunchReadinessInput as I};
        let dir = tempfile::tempdir().unwrap();
        for source in [
            Some("folder_alias"),
            Some(archivefs_core::MANUAL_PLATFORM_SOURCE),
        ] {
            let job = on_disk_from(&dir, "20000 Leagues.hdf", "Amiga", source, &[0u8; 4096]);
            if let Ok((I::IdentityUnknown(reason), _)) =
                build_input(&env(), &job, &AtomicBool::new(false))
            {
                assert_ne!(reason, R::AmigaHardDisk, "{source:?}");
            }
        }
        for (name, expected) in [("Game.ipf", R::AmigaIpf)] {
            for source in [
                Some("folder_alias"),
                Some(archivefs_core::MANUAL_PLATFORM_SOURCE),
            ] {
                let job = on_disk_from(&dir, name, "Amiga", source, &[0u8; 4096]);
                match build_input(&env(), &job, &AtomicBool::new(false)) {
                    Ok((I::IdentityUnknown(reason), _)) => assert_eq!(reason, expected, "{name}"),
                    Ok((I::Plan { .. }, _)) => panic!("{name} {source:?} planned"),
                    _ => panic!("{name} {source:?}: unexpected input"),
                }
            }
        }
    }

    /// Before 2026-11-09 every zipped cartridge in Blue stopped at "No
    /// runnable game file has been resolved" (13 systems in the census).
    #[test]
    fn a_zip_with_one_verified_cartridge_resolves_its_content_without_a_live_record() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let mut rom = vec![0u8; 0x8000];
        let logo = [
            0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B, 0x03, 0x73, 0x00, 0x83, 0x00, 0x0C,
            0x00, 0x0D, 0x00, 0x08, 0x11, 0x1F, 0x88, 0x89, 0x00, 0x0E, 0xDC, 0xCC, 0x6E, 0xE6,
            0xDD, 0xDD, 0xD9, 0x99, 0xBB, 0xBB, 0x67, 0x63, 0x6E, 0x0E, 0xEC, 0xCC, 0xDD, 0xDC,
            0x99, 0x9F, 0xBB, 0xB9, 0x33, 0x3E,
        ];
        rom[0x104..0x134].copy_from_slice(&logo);
        rom[0x14D] = archivefs_core::gb_header_evidence::compute_header_checksum(&rom).unwrap();
        let path = dir.path().join("Game (World).zip");
        {
            let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            writer
                .start_file("Game (World).gb", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(&rom).unwrap();
            writer.finish().unwrap();
        }
        let mut archive = archive(41, "Game Boy");
        archive.absolute_path = path.clone();
        archive.relative_path = "Game (World).zip".into();
        archive.archive_kind = "zip".into();
        let job = Job::for_game(&Game::from_archive(archive));
        let plan = planned(&job).expect("a launch plan");
        // No RetroArch is installed in the test environment, so the only
        // remaining content blocker is the "RetroArch only" route note.
        assert!(
            plan.candidates
                .iter()
                .all(|c| c.blockers.iter().all(|b| b.kind
                    != archivefs_core::launch::LaunchBlockerKind::ContentNotResolved
                    || b.detail.contains("RetroArch only"))),
            "content must resolve to the zip for extraction at Play"
        );
        assert!(
            plan.candidates
                .iter()
                .all(|c| c.content.resolved_path.as_deref() == Some(path.as_path()))
        );
    }

    #[test]
    fn a_format_the_system_does_not_list_still_needs_review() {
        let dir = tempfile::tempdir().unwrap();
        let job = on_disk(&dir, "notes.txt", "Philips CD-i", b"hello");
        assert!(planned(&job).is_none());
    }

    #[test]
    fn a_library_generation_change_drops_cached_answers_for_disc_set_members() {
        let g = game(8, "PlayStation");
        let mut state = super::super::browse_play::BrowsePlayState::default();
        state.note_generation_for_test(Some(1));
        state.record_readiness(&g, Some(C::Ready));
        assert!(state.is_ready_for_test(&g));
        // Same generation: nothing is touched.
        state.note_generation_for_test(Some(1));
        assert!(state.is_ready_for_test(&g));
        // A rescan (new generation) may have added Disc 2: the old Ready goes.
        state.note_generation_for_test(Some(2));
        assert!(!state.is_ready_for_test(&g));
    }

    #[test]
    fn dragon_coco_is_assessed_in_the_background_not_skipped() {
        assert!(!selection_bound_platform("Dragon / Tandy CoCo"));
        assert!(!selection_bound_platform("Dragon"));
        let cancel = AtomicBool::new(false);
        let job = Job::for_game(&game(7, "Dragon / Tandy CoCo"));
        // Unreadable here, so no verdict: never skipped, never Ready.
        assert_eq!(check(&env(), &job, &cancel), Outcome::Unknown);
    }

    #[test]
    fn a_background_xroar_context_is_visible_to_the_builder_for_its_path() {
        use crate::emulator_setup::xroar_cassette::Controller;
        use crate::launch_readiness_page::xroar::Context;
        let path = PathBuf::from("/fixture/game.cas");
        let context = Context {
            selected: path.clone(),
            result: Err(archivefs_core::launch::DragonCocoRefusal {
                kind: archivefs_core::launch::DragonCocoRefusalKind::Unreadable,
                detail: "fixture".into(),
            }),
        };
        let controller = Controller::with_context(path.clone(), context);
        assert!(controller.context(Some(&path)).is_some());
        assert!(
            controller
                .context(Some(Path::new("/fixture/other.cas")))
                .is_none()
        );
    }

    #[test]
    fn typed_identity_platforms_match_the_skip_rule() {
        use archivefs_core::game_identity::IdentityPlatform as P;
        for platform in [
            P::Dreamcast,
            P::Commodore64,
            P::WiiU,
            P::AtariST,
            P::Nes,
            P::PlayStation4,
        ] {
            assert!(selection_bound_identity(platform), "{platform:?}");
        }
        assert!(!selection_bound_identity(P::Snes));
        assert!(!selection_bound_identity(P::PlayStation));
    }

    #[test]
    fn the_remembered_emulator_choice_reaches_the_worker_state() {
        let mut snapshot = env();
        snapshot.remembered = vec![RememberedEmulatorProfile {
            adapter: "dolphin".into(),
            profile_id: "profile-a".into(),
            root: "/fixture/dolphin".into(),
        }];
        let state = snapshot.to_state();
        assert_eq!(state.remembered_emulator_profiles.len(), 1);
        assert_eq!(
            state.remembered_emulator_profiles[0].profile_id,
            "profile-a"
        );
    }

    #[test]
    fn every_selection_bound_platform_family_is_skipped_never_judged() {
        let cancel = AtomicBool::new(false);
        // The four approved families, by their aliases and canonical names,
        // plus the three same-kind snapshots (Atari ST, NES, PS4).
        for platform in [
            "Dreamcast",
            "Commodore 64",
            "C64",
            "Wii U",
            "WiiU",
            "Atari ST",
            "NES",
            "PS4",
        ] {
            let job = Job::for_game(&game(1, platform));
            assert_eq!(
                check(&env(), &job, &cancel),
                Outcome::Skipped,
                "{platform} must be left Not checked"
            );
        }
    }

    #[test]
    fn a_multi_disc_game_is_never_answered_in_the_background() {
        use crate::multidisc_launch_view::tests::{catalogue_archives, temp_root, verified_set};
        let cancel = AtomicBool::new(false);
        let root = temp_root("bg-disc-set");
        let (set, paths) = verified_set(&root, 2);
        let archives = catalogue_archives(&paths);
        // The foreground lookup finds a disc set for either disc...
        assert!(in_disc_set(
            std::slice::from_ref(&set),
            &paths[1],
            &archives
        ));
        // ...and not for an unrelated file, so single-disc games are unaffected.
        assert!(!in_disc_set(&[set], &root.join("other.cue"), &archives));
        // A readable disc of that set, flagged as the scheduler flags it,
        // yields no verdict at all: not Ready, not Not ready.
        let mut persisted = archives[1].clone();
        persisted.platform = Some("PlayStation".into());
        let mut job = Job::for_game(&Game::from_archive(persisted));
        job.multidisc = true;
        assert_eq!(check(&env(), &job, &cancel), Outcome::Skipped);
        // Without the flag the same game is judged (here: no answer without
        // finished emulator scans), proving the flag is what withholds it.
        job.multidisc = false;
        assert_ne!(check(&env(), &job, &cancel), Outcome::Skipped);
    }

    #[test]
    fn an_unreadable_game_is_unknown_not_ready_or_not_ready() {
        let cancel = AtomicBool::new(false);
        let job = Job::for_game(&game(2, "SNES"));
        assert_eq!(check(&env(), &job, &cancel), Outcome::Unknown);
    }

    #[test]
    fn skipped_and_unknown_outcomes_leave_games_not_checked() {
        let g = game(3, "Dreamcast");
        let mut cache = ReadinessCache::default();
        record_outcome(&mut cache, &g, Outcome::Skipped);
        record_outcome(&mut cache, &g, Outcome::Unknown);
        assert_eq!(cache.get(&g), None);
        record_outcome(&mut cache, &g, Outcome::Known(C::Ready));
        assert_eq!(cache.get(&g), Some(C::Ready));
    }

    #[test]
    fn a_background_answer_never_replaces_a_foreground_one() {
        let g = game(4, "SNES");
        let mut cache = ReadinessCache::default();
        cache.record(&g, Some(C::NotReady("Needs emulator")));
        record_outcome(&mut cache, &g, Outcome::Known(C::Ready));
        assert_eq!(cache.get(&g), Some(C::NotReady("Needs emulator")));
    }

    #[test]
    fn pick_is_bounded_by_capacity_candidates_and_settled_games() {
        let games: Vec<Game> = (0..20_000).map(|i| game(i, "SNES")).collect();
        let mut scheduler = Scheduler::default();
        let picked = scheduler.pick(games.iter(), |_| false);
        assert_eq!(picked.len(), MAX_IN_FLIGHT);
        assert_eq!(picked[0].id, 0, "nearest the top first");
        // Full pool: nothing more starts.
        scheduler.in_flight = MAX_IN_FLIGHT;
        assert!(scheduler.pick(games.iter(), |_| false).is_empty());
        // Games already decided or already cached are not picked again.
        scheduler.in_flight = 0;
        scheduler.settled.insert(0, Outcome::Skipped);
        let picked = scheduler.pick(games.iter(), |g| g.archive.id == 1);
        assert_eq!(picked.iter().map(|j| j.id).collect::<Vec<_>>(), [2, 3]);
        // Only the first MAX_CANDIDATES drawn are ever considered.
        let all_cached = scheduler.pick(games.iter(), |g| g.archive.id < MAX_CANDIDATES as i64);
        assert!(
            all_cached.is_empty(),
            "games past the window are not looked at"
        );
    }

    #[test]
    fn rapid_scrolling_never_exceeds_the_pool_or_requeues_a_game() {
        let games: Vec<Game> = (0..500).map(|i| game(i, "SNES")).collect();
        let mut scheduler = Scheduler::default();
        let mut started = std::collections::HashSet::new();
        for window in games.windows(MAX_CANDIDATES).step_by(7) {
            for job in scheduler.pick(window.iter(), |_| false) {
                assert!(started.insert(job.id), "game {} queued twice", job.id);
                scheduler.in_flight += 1;
                scheduler.settled.insert(job.id, Outcome::Unknown);
            }
            assert!(scheduler.in_flight() <= MAX_IN_FLIGHT);
            // A finished check frees one slot as the viewport moves on.
            scheduler.in_flight = scheduler.in_flight.saturating_sub(1);
        }
    }

    #[test]
    fn invalidation_cancels_outstanding_work_and_drops_its_late_results() {
        let mut scheduler = Scheduler::default();
        let cancel = Arc::clone(&scheduler.cancel);
        let old_epoch = scheduler.epoch();
        scheduler.in_flight = 1;
        scheduler.invalidate();
        assert!(cancel.load(Ordering::Relaxed), "old work is told to stop");
        assert_ne!(scheduler.epoch(), old_epoch);
        // A result from before the change arrives late: dropped, not applied.
        scheduler
            .sender
            .send(Finished {
                epoch: old_epoch,
                id: 9,
                outcome: Outcome::Known(C::Ready),
            })
            .unwrap();
        assert!(scheduler.drain().is_empty());
        assert_eq!(scheduler.in_flight(), 0, "the slot is still freed");
        // The new epoch's work is accepted.
        scheduler
            .sender
            .send(Finished {
                epoch: scheduler.epoch(),
                id: 10,
                outcome: Outcome::Known(C::Ready),
            })
            .unwrap();
        assert_eq!(scheduler.drain().len(), 1);
    }

    #[test]
    fn a_cancelled_check_returns_nothing() {
        let cancel = AtomicBool::new(true);
        // Even a game with real bytes yields no verdict once cancelled.
        let dir = std::env::temp_dir().join(format!("bg-readiness-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("x.sfc");
        std::fs::write(&file, vec![0u8; 4096]).unwrap();
        let mut archive = archive(5, "SNES");
        archive.absolute_path = file;
        let job = Job::for_game(&Game::from_archive(archive));
        assert_ne!(
            check(&env(), &job, &cancel),
            Outcome::Known(C::Ready),
            "a cancelled check must not report Ready"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
