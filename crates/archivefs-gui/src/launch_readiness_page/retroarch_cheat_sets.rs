//! Cheat sets already on this computer for the RetroArch game on screen.
//!
//! This reuses what Cheats & Mods already retrieved: the trusted Libretro cheat
//! catalogue in EmuWiz's cheat cache. Nothing here downloads, installs or
//! writes; the lookup only reads the cached catalogue (whose files are checked
//! against their recorded fingerprints by the cache inspection) and ranks its
//! files with the same candidate builder Cheats & Mods uses.
//!
//! A catalogue `.cht` file carries no fingerprint of the game it is for, so a
//! set matched by its name and platform is never treated as proven. Only a set
//! that declares this game's verified serial or content fingerprint is shown
//! as an exact match. Either way, previewing a set feeds the very same chooser
//! a hand-picked file uses: nothing is ticked, every cheat needs the player's
//! review, and the launch re-checks the cheat file, core and settings.
use super::retroarch_cheats::Choice;
use archivefs_core::emulator_environment::HostReadOnlyFilesystem;
use archivefs_core::launch::RetroArchLaunchRequest;
use archivefs_core::patch_manager::{
    CheatCandidate, CheatCandidateArchive, CheatCandidateClassification,
    CheatCandidateEvidenceKind, CheatCandidateOptions, CheatSourceFreshness,
    build_cheat_candidates, default_cheat_source_cache_root, inspect_retroarch_cheat_source,
    load_candidate_document, load_cheat_catalogue_snapshot, trusted_retroarch_cheat_sources,
};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

/// Shown whenever there is no usable local catalogue.
pub(super) const NO_CATALOGUE: &str = "No cheat catalogue has been retrieved on this computer yet. Get the Official Libretro cheat database in Cheats & Mods first; this page never downloads anything.";

/// What EmuWiz can and cannot say about the core's cheat support.
pub(super) fn core_mechanism_note(core_stem: &str) -> String {
    format!(
        "Cheat mechanism: RetroArch's own cheat codes, applied by the {core_stem} core. Before starting, EmuWiz asks the core file for its name and checks RetroArch's override files read-only. EmuWiz cannot tell in advance whether this core applies cheat codes, and RetroArch does not report it."
    )
}

/// How a local set relates to the game on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SetMatch {
    /// The set declares this game's verified serial or content fingerprint.
    Exact,
    /// Matched by name (and perhaps platform and region) only.
    Uncertain,
    /// For another platform or emulator, or unreadable. Never previewed.
    Incompatible,
}

impl SetMatch {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Exact => "Exact match (verified fingerprint)",
            Self::Uncertain => "Possible match — review required",
            Self::Incompatible => "Not compatible",
        }
    }
}

/// One catalogue file, as listed for this game.
#[derive(Clone, Debug)]
pub(super) struct LocalSet {
    pub(super) relative_path: String,
    pub(super) display_name: String,
    pub(super) cheat_count: usize,
    pub(super) matched: SetMatch,
    pub(super) reason: &'static str,
    source_file_hash: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct Found {
    pub(super) catalogue_root: PathBuf,
    pub(super) stale: bool,
    pub(super) sets: Vec<LocalSet>,
    pub(super) truncated: bool,
}

/// The identity the lookup ranks against: the verified launch identity only.
/// The content file's name is passed as the display name because a catalogue
/// is organised by name, but a name never makes a set exact.
fn archive_for(request: &RetroArchLaunchRequest) -> CheatCandidateArchive {
    let stem = request
        .selected_content_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned());
    let key = request.expected_game_key.trim();
    let is_sha256 = key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit());
    CheatCandidateArchive {
        display_name: stem.clone().unwrap_or_default(),
        platform: Some(request.expected_platform_id.clone()),
        region: None,
        serial: (!is_sha256 && !key.is_empty()).then(|| key.to_string()),
        content_hash: is_sha256.then(|| key.to_ascii_lowercase()),
        content_basename: stem,
    }
}

fn classify(candidate: &CheatCandidate) -> (SetMatch, &'static str) {
    use CheatCandidateClassification as C;
    if !candidate.manually_selectable {
        return match candidate.classification {
            C::CrossPlatform => (SetMatch::Incompatible, "made for a different platform"),
            _ => (
                SetMatch::Incompatible,
                "made for another emulator, or it could not be read cleanly",
            ),
        };
    }
    let fingerprint = candidate.evidence.iter().any(|e| {
        matches!(
            e.kind,
            CheatCandidateEvidenceKind::ExactSerial | CheatCandidateEvidenceKind::ExactContentHash
        )
    });
    let platform_conflict = candidate
        .evidence
        .iter()
        .any(|e| e.kind == CheatCandidateEvidenceKind::PlatformMismatch);
    if fingerprint && !platform_conflict && candidate.classification != C::Ambiguous {
        return (
            SetMatch::Exact,
            "the set declares this game's verified serial or content fingerprint",
        );
    }
    let reason = match candidate.classification {
        C::Ambiguous => "several sets match equally well; check the name and region",
        C::Weak => "matched by name only, without the platform agreeing",
        _ => "matched by name and platform only; the set carries no fingerprint of this game",
    };
    (SetMatch::Uncertain, reason)
}

/// Ranks the files of an already-located catalogue for `request`. Read-only.
pub(super) fn find_in_catalogue(
    request: &RetroArchLaunchRequest,
    catalogue_root: &Path,
    stale: bool,
) -> Found {
    let snapshot =
        load_cheat_catalogue_snapshot(&HostReadOnlyFilesystem, "trusted-catalogue", catalogue_root);
    let list = build_cheat_candidates(
        &snapshot,
        &archive_for(request),
        &CheatCandidateOptions::default(),
    );
    let mut sets: Vec<LocalSet> = list
        .candidates
        .iter()
        .map(|candidate| {
            let (matched, reason) = classify(candidate);
            LocalSet {
                relative_path: candidate.catalogue_relative_path.clone(),
                display_name: candidate.display_name.clone(),
                cheat_count: candidate.cheat_count,
                matched,
                reason,
                source_file_hash: candidate.source_file_hash.clone(),
            }
        })
        .collect();
    // Exact first, then uncertain, then incompatible; the builder's own order
    // (best first) is kept within each group.
    sets.sort_by_key(|set| match set.matched {
        SetMatch::Exact => 0,
        SetMatch::Uncertain => 1,
        SetMatch::Incompatible => 2,
    });
    Found {
        catalogue_root: catalogue_root.to_path_buf(),
        stale,
        sets,
        truncated: list.truncated,
    }
}

/// Locates the cached trusted catalogue under `cache_root` and ranks it.
/// Never fetches: a missing or unusable cache is reported, not repaired.
pub(super) fn discover(
    request: &RetroArchLaunchRequest,
    cache_root: &Path,
) -> Result<Found, String> {
    for source in trusted_retroarch_cheat_sources() {
        let inspection = inspect_retroarch_cheat_source(&source.source_id, cache_root).map_err(
            |e| {
                format!(
                    "The local cheat catalogue cannot be used ({}). Refresh it in Cheats & Mods. Nothing was changed.",
                    e.message
                )
            },
        )?;
        let Some(path) = inspection
            .current_catalogue_path
            .filter(|_| inspection.setup_usable)
        else {
            continue;
        };
        if path.lossy {
            return Err(
                "The local cheat catalogue's folder name cannot be read safely. Nothing was changed."
                    .into(),
            );
        }
        let stale = inspection.freshness == CheatSourceFreshness::Stale;
        return Ok(find_in_catalogue(request, Path::new(&path.display), stale));
    }
    Err(NO_CATALOGUE.into())
}

/// Re-checks the listed file against the fingerprint taken when it was listed,
/// then opens it in the ordinary chooser. Writes nothing.
pub(super) fn preview(
    request: &RetroArchLaunchRequest,
    found: &Found,
    set: &LocalSet,
) -> Result<Choice, String> {
    if set.matched == SetMatch::Incompatible {
        return Err(format!(
            "\"{}\" cannot be used for this game: {}.",
            set.display_name, set.reason
        ));
    }
    load_candidate_document(
        &found.catalogue_root,
        &set.relative_path,
        set.source_file_hash.as_deref(),
    )
    .map_err(|_| {
        format!(
            "\"{}\" changed or disappeared since it was listed, so it was not opened. Look for cheat sets again.",
            set.display_name
        )
    })?;
    Choice::load(request, &found.catalogue_root.join(&set.relative_path))
}

/// The key a lookup belongs to: a result for another game is never shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Key {
    content: PathBuf,
    platform: String,
    game_key: String,
}

impl Key {
    fn of(request: &RetroArchLaunchRequest) -> Self {
        Self {
            content: request.selected_content_path.clone(),
            platform: request.expected_platform_id.clone(),
            game_key: request.expected_game_key.clone(),
        }
    }
}

#[derive(Default)]
pub(super) enum Lookup {
    #[default]
    NotChecked,
    Checking(Key, Receiver<Result<Found, String>>),
    Done(Key, Result<Found, String>),
}

impl Lookup {
    pub(super) fn start(&mut self, request: &RetroArchLaunchRequest, cache_root: Option<PathBuf>) {
        let (sender, receiver) = mpsc::channel();
        let worker_request = request.clone();
        std::thread::spawn(move || {
            let result = match cache_root.map_or_else(
                || default_cheat_source_cache_root().map_err(|_| NO_CATALOGUE.to_string()),
                Ok,
            ) {
                Ok(root) => discover(&worker_request, &root),
                Err(error) => Err(error),
            };
            let _ = sender.send(result);
        });
        *self = Self::Checking(Key::of(request), receiver);
    }

    /// Non-blocking. Returns whether a lookup is still running.
    pub(super) fn poll(&mut self) -> bool {
        let Self::Checking(key, receiver) = self else {
            return false;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => {
                Err("The cheat-set lookup stopped without a result.".into())
            }
        };
        *self = Self::Done(key.clone(), result);
        false
    }

    /// Forgets a lookup made for a different game.
    pub(super) fn retain_for(&mut self, request: &RetroArchLaunchRequest) {
        let key = Key::of(request);
        let stale = match self {
            Self::NotChecked => false,
            Self::Checking(k, _) | Self::Done(k, _) => *k != key,
        };
        if stale {
            *self = Self::NotChecked;
        }
    }

    pub(super) fn result(&self) -> Option<&Result<Found, String>> {
        match self {
            Self::Done(_, result) => Some(result),
            _ => None,
        }
    }

    pub(super) fn is_checking(&self) -> bool {
        matches!(self, Self::Checking(..))
    }
}

/// The "already on this computer" part of the cheat chooser.
pub(super) fn show(
    ui: &mut eframe::egui::Ui,
    state: &mut super::RetroArchLaunchState,
    request: &RetroArchLaunchRequest,
) {
    ui.small(core_mechanism_note(&request.core_stem));
    state.cheat_sets.retain_for(request);
    if state.cheat_sets.poll() {
        ui.ctx().request_repaint();
    }
    if state.cheat_sets.is_checking() {
        ui.label("Looking for cheat sets on this computer…");
        return;
    }
    let mut look = false;
    let mut chosen: Option<LocalSet> = None;
    match state.cheat_sets.result() {
        None => {
            look = ui.button("Look for cheat sets on this computer").clicked();
        }
        Some(Err(message)) => {
            ui.label(message.as_str());
            if ui.button("Open Cheats & Mods").clicked() {
                state.open_cheats_mods = true;
            }
            look = ui.button("Look again").clicked();
        }
        Some(Ok(found)) => {
            if found.stale {
                ui.small("The local cheat catalogue is out of date. Refresh it in Cheats & Mods for newer cheats.");
            }
            if found.sets.is_empty() {
                ui.label("No cheat set on this computer matches this game.");
            }
            for set in &found.sets {
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!(
                        "{} · {} · {} cheat(s)",
                        set.matched.label(),
                        set.display_name,
                        set.cheat_count
                    ));
                    if set.matched != SetMatch::Incompatible && ui.button("Preview").clicked() {
                        chosen = Some(set.clone());
                    }
                });
                ui.small(format!("  {}.", set.reason));
            }
            if found.truncated {
                ui.small("Only the closest matches are listed.");
            }
            if ui.button("Open Cheats & Mods").clicked() {
                state.open_cheats_mods = true;
            }
            look = ui.button("Look again").clicked();
        }
    }
    if let (Some(set), Some(Ok(found))) = (chosen, state.cheat_sets.result()) {
        match preview(request, found, &set) {
            Ok(choice) => {
                state.cheats = Some(choice);
                state.cheats_message = None;
            }
            Err(error) => {
                state.cheats = None;
                state.cheats_message = Some(error);
            }
        }
    }
    if look {
        state
            .cheat_sets
            .start(request, state.cheat_cache_root.clone());
    }
}
