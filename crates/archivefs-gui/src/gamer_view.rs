//! Gamer View: the simple, one-screen front door (search + platform chips +
//! game list + selected-game action panel) - see
//! docs/GUI_NAVIGATION_RESET_DESIGN.md. Extracted verbatim from `main.rs`
//! (2026-08-22, GUI extraction Phase A); `GuiMode` itself (which of Gamer
//! View/Advanced View is active) stays in `main.rs` as top-level app state,
//! since the top-level render dispatch reads it directly - only Gamer
//! View's own screen state (`GamerViewScreen`) and rendering moved here.

use super::*;

/// Which of Gamer View's two screens is currently showing - the
/// one-screen game list (default) or the read-only Details view reached
/// from the selected-game action panel. Never persisted; always starts
/// on `GameList`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum GamerViewScreen {
    #[default]
    GameList,
    Details,
}

/// The small identity vocabulary used by Gamer View's Details screen. The
/// underlying evidence verdict remains authoritative; these labels only make
/// an already-available result understandable without exposing its internal
/// evidence terminology.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GamerIdentityStatus {
    Identified,
    Uncertain,
    ConflictingEvidence,
    StillChecking,
}

impl GamerIdentityStatus {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Identified => "Identified",
            Self::Uncertain => "Uncertain",
            Self::ConflictingEvidence => "Conflicting evidence",
            Self::StillChecking => "Still checking",
        }
    }
}

pub(crate) fn gamer_identity_status_from_verdict(
    status: archivefs_core::platform_evidence_fusion::identity_presentation::IdentityStatus,
) -> GamerIdentityStatus {
    use archivefs_core::platform_evidence_fusion::identity_presentation::IdentityStatus;

    match status {
        IdentityStatus::Conflict => GamerIdentityStatus::ConflictingEvidence,
        IdentityStatus::Ambiguous | IdentityStatus::Unknown => GamerIdentityStatus::Uncertain,
        IdentityStatus::VerifiedByDat
        | IdentityStatus::ContentAndDatAgree
        | IdentityStatus::ContentOnly
        | IdentityStatus::DatOnly => GamerIdentityStatus::Identified,
    }
}

pub(crate) const GAMER_SEARCH_MIN_WIDTH: f32 = 320.0;
pub(crate) const GAMER_SEARCH_MAX_WIDTH: f32 = 760.0;
pub(crate) const GAMER_TOP_BAR_CONTROL_RESERVE: f32 = 64.0;

/// Gives search the top bar's main share while reserving enough room for
/// the settings menu (and the busy spinner when present). The final `min`
/// keeps unusually small windows from pushing that control off-screen.
pub(crate) fn gamer_search_width(available_width: f32) -> f32 {
    let available_for_search = (available_width - GAMER_TOP_BAR_CONTROL_RESERVE).max(0.0);
    available_for_search
        .clamp(GAMER_SEARCH_MIN_WIDTH, GAMER_SEARCH_MAX_WIDTH)
        .min(available_for_search)
}
/// Gamer View's own wording for the scan its "Add games" flow just
/// chained - never the Advanced-View "Scan complete: N source(s)
/// scanned, N archive(s) found" phrasing `source_action_success_message`
/// uses, which names "source(s)"/"archive(s)" directly. Reads the exact
/// same already-computed `ingestion_stats` Collection Discovery and the
/// Advanced View Sources banner already read - no detection logic is
/// duplicated, only the presentation.
pub(crate) fn gamer_view_first_scan_message(summary: &ScanPersistSummary) -> String {
    let stats = &summary.ingestion_stats;
    let found = stats.loose_roms
        + stats.disc_images
        + stats.amiga_images
        + stats.computer_disks
        + stats.game_folders
        + stats.archives;
    if found == 0 {
        return "We looked through that folder but didn't find any games in it. \
                Double-check it's the right folder, or try another one."
            .to_string();
    }
    let plural = if found == 1 { "game" } else { "games" };
    let skipped = summary.skipped_files_total();
    if skipped > 0 {
        return format!(
            "Found {found} {plural}. {skipped} file{} need attention.",
            if skipped == 1 { "" } else { "s" }
        );
    }
    let ingestion_skipped = summary.ingestion_skip_reasons.total();
    if ingestion_skipped > 0 {
        return format!(
            "Found {found} {plural}. {ingestion_skipped} item{} need attention.",
            if ingestion_skipped == 1 { "" } else { "s" }
        );
    }
    if summary.counts.errors_count > 0 || !summary.folder_errors.is_empty() {
        return format!("Found {found} {plural}. Some folders need attention.");
    }
    format!("Found {found} {plural}!")
}

/// Whether the completed scan has existing, truthful detail that the user can
/// review. This only consults counters already supplied by the scan; it does
/// not infer a count from the library rows.
pub(crate) fn gamer_view_scan_needs_review(summary: &ScanPersistSummary) -> bool {
    summary.skipped_files_total() > 0
        || summary.ingestion_skip_reasons.total() > 0
        || summary.counts.errors_count > 0
        || !summary.folder_errors.is_empty()
}

/// Beginner-facing failure copy for Gamer View's explicit folder-add action.
/// The precise backend/config/I/O error is retained separately in
/// `ActionFeedback::more_information` and rendered under Technical details.
pub(crate) const GAMER_ADD_GAMES_FAILURE_MESSAGE: &str =
    "EmuWiz couldn't finish adding that game folder, so no scan was started.";
/// Gamer View's plain-language primary-action state for a selected game -
/// see docs/GUI_NAVIGATION_RESET_DESIGN.md §2.3. Never exposes a raw
/// `MountState` variant name (finding #4); Advanced View keeps the
/// precise names unchanged (`mount_validation_label`, `planned_action_label`,
/// `MountState`'s own `Display` impl - none of those are touched here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GamerPrimaryAction {
    Mount,
    Unmount,
    NoMountingNeeded,
    Blocked(String),
}

pub(crate) fn gamer_primary_action(state: MountState) -> GamerPrimaryAction {
    match state {
        MountState::Pending => GamerPrimaryAction::Mount,
        MountState::Mounted => GamerPrimaryAction::Unmount,
        MountState::NotMountable => GamerPrimaryAction::NoMountingNeeded,
        MountState::MountPathExists => GamerPrimaryAction::Blocked(
            "A file already exists where EmuWiz would prepare this game. Open Advanced View to \
             resolve this."
                .to_string(),
        ),
    }
}

/// The short, list-row-sized counterpart of `gamer_primary_action` - one
/// or two words, safe to append after a title/platform pair. The list has
/// no per-row launch plan, so a mount-free game reads only "Game found"
/// (media is usable); whether a safe core exists is the selected-game card's
/// verdict ("Ready to play" vs "Needs setup"), never claimed here.
pub(crate) fn gamer_primary_action_short_label(action: &GamerPrimaryAction) -> &'static str {
    match action {
        GamerPrimaryAction::Mount => "Prepare game",
        GamerPrimaryAction::Unmount => "Mounted",
        GamerPrimaryAction::NoMountingNeeded => "Game found",
        GamerPrimaryAction::Blocked(_) => "Needs attention",
    }
}

/// The Gamer View card's **single** reconciled readiness state, derived from
/// media/mount readiness *and* safe-launch readiness together. The card's
/// status line and its primary action both consume this one value, so a
/// game can never simultaneously read "Ready to play" and "Can't play yet".
///
/// A game that needs no mounting is only `Ready` when the shared launch
/// planner ([`launch_readiness_page::gamer_play_action`]) actually produced
/// a safe RetroArch request; a blocked plan is `NeedsSetup`, never
/// "Ready to play". No launch-planning rule is re-implemented here.
pub(crate) enum GamerReadiness<'a> {
    /// Pending mount - the media is usable once mounted.
    Mount,
    /// A container needs a bounded read-only preparation pass before it can
    /// expose Play.
    Prepare,
    /// More than one safe member was found; the user must choose one.
    ChooseMember {
        candidates: &'a [archivefs_core::PreparedMemberCandidate],
    },
    /// Currently mounted.
    Unmount,
    /// Media usable and a safe launch plan exists.
    Ready {
        request: &'a launch_readiness_page::TypedLaunchRequest,
    },
    /// Media usable, but the launch is blocked. The typed blocker supplies
    /// both the novice heading and the correct next action.
    NeedsSetup {
        blocker: &'a launch_readiness_page::GamerBlocker,
    },
    /// The media / mount itself is blocked. Carries the mount blocker.
    NeedsAttention { reason: String },
}

/// Reconciles mount state and the shared launch-plan projection into one
/// [`GamerReadiness`]. `MountState` alone never yields `Ready`.
pub(crate) fn gamer_readiness<'a>(
    mount_state: MountState,
    play_action: &'a launch_readiness_page::GamerPlayAction,
) -> GamerReadiness<'a> {
    match gamer_primary_action(mount_state) {
        GamerPrimaryAction::Mount => GamerReadiness::Mount,
        GamerPrimaryAction::Unmount => GamerReadiness::Unmount,
        GamerPrimaryAction::Blocked(reason) => GamerReadiness::NeedsAttention { reason },
        GamerPrimaryAction::NoMountingNeeded => match play_action {
            launch_readiness_page::GamerPlayAction::Launch(request) => {
                GamerReadiness::Ready { request }
            }
            launch_readiness_page::GamerPlayAction::BlockedTyped(blocker) => {
                GamerReadiness::NeedsSetup { blocker }
            }
        },
    }
}

/// Maps the shared launch planner's typed projection onto a
/// [`GamerReadiness`]. Play is offered only when the planner actually
/// produced a safe request; a typed blocker becomes `NeedsSetup`, which
/// routes to the correct setup action - never back to Prepare.
fn planner_readiness(play_action: &launch_readiness_page::GamerPlayAction) -> GamerReadiness<'_> {
    match play_action {
        launch_readiness_page::GamerPlayAction::Launch(request) => {
            GamerReadiness::Ready { request }
        }
        launch_readiness_page::GamerPlayAction::BlockedTyped(blocker) => {
            GamerReadiness::NeedsSetup { blocker }
        }
    }
}

/// Archive containers have one extra, explicit preparation step. Mount state
/// alone is never treated as launch readiness: Play is available only after a
/// specific member has been retained and the shared planner accepts it.
pub(crate) fn gamer_archive_readiness<'a>(
    mount_state: MountState,
    prepared: bool,
    play_action: &'a launch_readiness_page::GamerPlayAction,
    member_choices: Option<&'a [archivefs_core::PreparedMemberCandidate]>,
) -> GamerReadiness<'a> {
    match gamer_primary_action(mount_state) {
        GamerPrimaryAction::Blocked(reason) => GamerReadiness::NeedsAttention { reason },
        // An explicit multi-member chooser always wins: the user must pick
        // one member before any launch plan can be trusted.
        GamerPrimaryAction::Mount | GamerPrimaryAction::Unmount if member_choices.is_some() => {
            GamerReadiness::ChooseMember {
                candidates: member_choices.expect("checked above"),
            }
        }
        // Not mounted yet: the bounded preparation pass still has to run. A
        // leftover `prepared` flag from a previous selection cannot stand in
        // for a live mount.
        GamerPrimaryAction::Mount => GamerReadiness::Prepare,
        // Mounted, but no compatible member has been retained yet.
        GamerPrimaryAction::Unmount if !prepared => GamerReadiness::Prepare,
        // Mounted with exactly one retained compatible member: the shared
        // launch planner has already evaluated that exact member (its inner
        // path is fed through `build_launch_readiness_input`), so defer to
        // it just like a mount-free game. Successful retention therefore
        // reaches Play - or the correct typed blocker - instead of looping
        // back to Prepare.
        GamerPrimaryAction::Unmount => planner_readiness(play_action),
        GamerPrimaryAction::NoMountingNeeded => planner_readiness(play_action),
    }
}

/// The one- or two-word status word for a [`GamerReadiness`] - the text on
/// the card's status line, kept in lockstep with its primary action.
pub(crate) fn gamer_readiness_short_label(readiness: &GamerReadiness<'_>) -> &'static str {
    match readiness {
        GamerReadiness::Mount => "Prepare game",
        GamerReadiness::Prepare => "Ready to prepare",
        GamerReadiness::ChooseMember { .. } => "Choose game file",
        GamerReadiness::Unmount => "Mounted",
        GamerReadiness::Ready { .. } => "Ready to play",
        GamerReadiness::NeedsSetup { .. } => "Needs setup",
        GamerReadiness::NeedsAttention { .. } => "Needs attention",
    }
}

/// Keeps the planner's exact refusal available without exposing backend
/// terminology in the main sentence, and exposes the blocker-specific next
/// action to the caller through the returned `GamerViewAction`.
fn show_gamer_launch_blocker(ui: &mut egui::Ui, blocker: &launch_readiness_page::GamerBlocker) {
    ui.colored_label(ui.visuals().warn_fg_color, blocker.heading());
    let next = match blocker.kind {
        launch_readiness_page::GamerBlockerKind::CheckingGame => {
            "EmuWiz is still checking this game."
        }
        launch_readiness_page::GamerBlockerKind::UnknownSystem => {
            "Review the game identity and assign its system if you know it."
        }
        launch_readiness_page::GamerBlockerKind::ConflictingIdentity => {
            "Review the conflicting identity evidence before choosing a system."
        }
        launch_readiness_page::GamerBlockerKind::ContentNeedsPreparation => {
            "Prepare the game's content, then return here to play."
        }
        launch_readiness_page::GamerBlockerKind::EmulatorNotInstalled => {
            "Open Emulator Setup to install or register this emulator."
        }
        launch_readiness_page::GamerBlockerKind::EmulatorSetupIncomplete => {
            "Open Emulator Setup to finish this emulator's setup."
        }
        launch_readiness_page::GamerBlockerKind::EmulatorNotChecked => {
            "Run the emulator check to see what is ready on this computer."
        }
        launch_readiness_page::GamerBlockerKind::NoSafeEmulator => {
            "Run the emulator check or review launch readiness for details."
        }
        launch_readiness_page::GamerBlockerKind::MultipleChoices => {
            "Choose one emulator explicitly before playing."
        }
        launch_readiness_page::GamerBlockerKind::LaunchPlanInvalid => {
            "Review launch readiness for the exact reason this plan was refused."
        }
    };
    ui.label(next);
    widgets::technical_details(ui, "gamer-launch-blocker", |ui| {
        ui.label(&blocker.detail);
    });
}

const GAMER_PLAY_LABEL: &str = "Play";
const COPY_FOLDER_LOCATION_LABEL: &str = "Copy folder location";

fn gamer_copy_location_label() -> &'static str {
    COPY_FOLDER_LOCATION_LABEL
}

/// Prefers the real, human title from metadata; falls back to the
/// archive's filename (without extension) rather than ever showing a raw
/// internal path as if it were a title.
pub(crate) fn gamer_display_title(record: &ArchiveRecord) -> String {
    record
        .metadata
        .title
        .clone()
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| {
            record
                .mount_plan
                .archive
                .path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("Unknown game")
                .to_string()
        })
}

/// Stage 3's exact rule: Undo is shown only when the selected game has a
/// genuinely reversible operation recorded - never speculatively. This
/// only gates *visibility*; the click still routes through
/// `start_cheat_install_rollback`, which carries its own authoritative
/// safety checks unchanged.
pub(crate) fn gamer_undo_available(
    workflow: Option<&CheatWorkflowState>,
    selected: Option<&Path>,
) -> bool {
    let (Some(workflow), Some(selected)) = (workflow, selected) else {
        return false;
    };
    workflow.archive_path == selected
        && matches!(workflow.transaction, CheatTransactionState::Result { .. })
}

pub(crate) enum GamerViewAction {
    Operation(OperationRequest),
    Prepare(PathBuf),
    SelectArchiveMember(PathBuf, String),
    Play(Box<launch_readiness_page::TypedLaunchRequest>),
    OpenCheatsMods(PathBuf),
    /// The folder to copy to the clipboard - "Copy folder location" (§2.1's
    /// action visibility rules). No file-manager process is launched;
    /// see the implementation summary for why (no such capability, and
    /// no new external-process dependency, exists in this codebase today).
    CopyLocation(String),
    Undo,
    /// First-run "Add games": the folder a person just picked via the
    /// native folder dialog (opened synchronously inside `show_gamer_view`,
    /// matching every other `rfd::FileDialog` call site in this codebase).
    /// The caller reuses the exact same `SourceAction::Add` the Advanced
    /// View Sources page uses - see `gamer_view_pending_first_scan` for
    /// how the resulting scan is chained automatically.
    AddGamesFolder(PathBuf),
    /// Open the existing Sources -> Discovery review for the most recent
    /// scan, without starting another scan or changing any file.
    ReviewScan,
    /// Reuses the existing `SourceAction::ScanAll` path. Gamer View owns only
    /// the beginner-facing entry point; source enumeration, cancellation, and
    /// scan persistence remain in the shared background action.
    ScanForNewGames,
    /// Phase 5: "Review" on a game whose platform couldn't be confidently
    /// identified - the archive to keep selected while switching into
    /// Advanced View's Selected page, which already shows the real
    /// identity/evidence detail. Mirrors `OpenCheatsMods`'s exact
    /// select-then-navigate shape (`AppOperationRequest::OpenCheatsMods`'s
    /// handler) rather than inventing new plumbing.
    ReviewIdentity(PathBuf),
    /// Open the selected game's launch/readiness surface where the user can
    /// choose one of several equally valid emulator candidates.
    OpenLaunchChoices(PathBuf),
    /// Run the existing Doctor emulator check before returning to setup.
    CheckEmulators(PathBuf),
    /// "Open Emulator Setup" from a `NeedsSetup` card: the game whose
    /// launch is blocked, plus the exact emulator repair card to bring into
    /// view. The caller keeps the game selected and switches to Advanced
    /// View's Emulator Setup page, the same select-then-navigate shape as
    /// `ReviewIdentity`. The focus is carried as typed navigation state,
    /// never derived by parsing blocker text.
    OpenEmulatorSetup(PathBuf, EmulatorSetupFocus),
    /// "Update game information": re-reads the already-cached enrichment
    /// data from disk for the currently focused game. Never itself
    /// contacts a metadata provider's server - see
    /// `crate::game_metadata`'s module doc comment for why - so it affects
    /// enrichment display only, never identity, mount state, or anything
    /// else about the game.
    RefreshGameInformation,
}

fn emulator_setup_focus(emulator: &str) -> EmulatorSetupFocus {
    if emulator.eq_ignore_ascii_case("RetroArch") {
        EmulatorSetupFocus::RetroArch
    } else {
        EmulatorSetupFocus::Emulator(emulator.to_string())
    }
}

/// The single authoritative row snapshot one Gamer View frame is built
/// from: the shelf's counts, the "All" count and the game list all read
/// from this one object, so they cannot disagree.
///
/// # Why this type exists
///
/// The shelf used to count every row in the library while the list below
/// it applied `LibraryRowFilters::matches` in full. `LibraryRowFilters` is
/// shared with Advanced View, which exposes five further checkboxes
/// (Present / Missing / Awaiting validation / Known platform / Unknown
/// platform) that Gamer View neither shows nor can clear. Ticking any of
/// them in Advanced View - or opening the Health dashboard's "Review
/// missing", which sets `missing` directly - and returning to Gamer View
/// left every card advertising its full count while the list matched
/// nothing, reporting "No games match the selected platform" for a
/// platform whose card said 4023. Only a restart cleared it, because
/// `LibraryRowFilters` is not persisted.
///
/// Gamer View exposes exactly two filters - the search box and the
/// platform shelf - so those are the only two it applies. `candidates` is
/// everything the search admits; the counts are derived from `candidates`,
/// and `visible` is `candidates` narrowed by the platform selection. A
/// non-zero card count therefore *is* the number of rows selecting that
/// card produces, by construction rather than by agreement.
pub(crate) struct GamerLibrarySnapshot {
    /// Indices into the row list that pass every filter Gamer View
    /// exposes *except* the platform selection. The "All" card's count.
    pub(crate) candidates: Vec<usize>,
    /// `candidates` narrowed by the platform selection - exactly what the
    /// game list renders.
    pub(crate) visible: Vec<usize>,
    /// Counted over `candidates`, never over the unfiltered library.
    pub(crate) platform_counts: DetectedPlatformCounts,
    /// A platform is selected that no card in this snapshot offers, so no
    /// card is highlighted and no click can restore the list. The caller
    /// resets the selection to All.
    pub(crate) selection_is_stale: bool,
}

impl GamerLibrarySnapshot {
    pub(crate) fn build(rows: &[ArchiveRow], platform: Option<&str>, search_text: &str) -> Self {
        let candidates: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| search_text.is_empty() || row.search_text.contains(search_text))
            .map(|(index, _)| index)
            .collect();
        let platform_counts = detected_platform_counts(
            candidates
                .iter()
                .map(|index| &rows[*index])
                .map(|row| (!row.unknown_platform).then_some(row.platform.as_str())),
        );
        let visible: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|index| gamer_row_matches_platform(&rows[*index], platform))
            .collect();
        // Judged against the whole library, never against `candidates`.
        // A search that currently matches nothing would otherwise make every
        // platform look absent and silently throw the user's selection away
        // as they typed - the search must narrow the list, not rewrite what
        // they picked. "Unknown" is a real platform here, so it is stale on
        // the same terms: only when nothing in the library is unclassified.
        let selection_is_stale = match platform {
            None => false,
            Some("Unknown") => !rows.iter().any(|row| row.unknown_platform),
            Some(platform) => !rows
                .iter()
                .any(|row| !row.unknown_platform && row.platform == platform),
        };
        Self {
            candidates,
            visible,
            platform_counts,
            selection_is_stale,
        }
    }
}

/// Why the game list is empty, in the user's terms. Search and platform
/// compose, so when both are narrowing the message says so rather than
/// blaming whichever one the old `else if` chain happened to test first.
pub(crate) fn gamer_empty_list_guidance(
    library_is_empty: bool,
    searching: bool,
    platform_selected: bool,
) -> &'static str {
    match (library_is_empty, searching, platform_selected) {
        (true, _, _) => "No games are in your library yet.",
        (false, true, true) => "No games on this platform match your search.",
        (false, true, false) => "No games match your search.",
        (false, false, true) => "No games match the selected platform.",
        (false, false, false) => "No games are in your library yet.",
    }
}

/// Whether Gamer View's first-run "Add games" call-to-action should show
/// alongside the empty-list guidance above - only the true first-run case
/// (nothing in the library at all, and no search/platform filter could be
/// hiding something that does exist), never "no results for your search"
/// or "no games on this platform" - those already have a different real
/// cause a folder-add button would not fix.
pub(crate) fn gamer_view_shows_add_games_button(
    library_is_empty: bool,
    search_is_empty: bool,
    no_platform_selected: bool,
) -> bool {
    library_is_empty && search_is_empty && no_platform_selected
}

/// Phase 5: a game whose platform couldn't be resolved is as much
/// "Needs attention" in the list as a blocked mount is - both get the
/// same word here; the selected-game panel explains which one applies
/// (and, for the platform case, offers Review) once the row is opened.
pub(crate) fn gamer_view_row_state_label(
    unknown_platform: bool,
    mount_state: MountState,
) -> &'static str {
    if unknown_platform {
        "Needs attention"
    } else {
        gamer_primary_action_short_label(&gamer_primary_action(mount_state))
    }
}

/// Phase 4: Gamer View's own wording for a failed action, translated from
/// the shared `ActionFeedback.message` every failure already sets -
/// itself untouched, since Advanced View's own feedback banners
/// (`self.feedback`, read at several other call sites) still show that
/// raw, precise text unchanged. Today every mount/unmount/scan failure
/// reaching that shared field is `ArchiveFsError`'s raw `Display` output
/// (core/src/lib.rs) - e.g. `"scanner error: ..."`, `"database error:
/// ..."`, or a bare OS error like `"/path: Permission denied (os error
/// 13)"` - none of it written for a beginner. This function classifies
/// the *shape* of that already-flattened string rather than requiring a
/// new error-classification type to be threaded through the mount
/// pipeline (out of this phase's "no backend changes" scope): permission
/// and not-found problems get a precise message when the raw text
/// recognisably names one; everything else - including a scanner/
/// database/config-flavored raw message, so those words are never
/// echoed here - gets one honest, generic explanation that still meets
/// every bar Phase 4 set: what happened, that the user's files are safe
/// (true for every mount/unmount/scan failure - none of them write to
/// or delete a game file), a real next action the app actually supports
/// (retry; Advanced View has the detail), and no invented certainty
/// about a cause this function cannot actually determine.
pub(crate) fn gamer_view_failure_message(raw: &str) -> String {
    if raw == GAMER_ADD_GAMES_FAILURE_MESSAGE {
        return raw.to_string();
    }
    let lower = raw.to_ascii_lowercase();
    if lower.contains("permission denied") {
        "EmuWiz doesn't have permission to access this file or folder. Check the folder's \
         permissions, then try again."
            .to_string()
    } else if lower.contains("no such file or directory") || lower.contains("not found") {
        "We can't find this any more - it may have been moved, renamed, or deleted. Your \
         other games are safe."
            .to_string()
    } else {
        "Something didn't work. Your games are safe - nothing was changed or deleted. Try \
         again, or check Advanced View for more detail."
            .to_string()
    }
}

/// The platform half of Gamer View's filtering, matching
/// `LibraryRowFilters::matches`'s own platform rule exactly - `"Unknown"`
/// is the diagnostic selection for unclassified rows, not an empty one.
pub(crate) fn gamer_row_matches_platform(row: &ArchiveRow, platform: Option<&str>) -> bool {
    platform.is_none_or(|wanted| {
        if wanted == "Unknown" {
            row.unknown_platform
        } else {
            !row.unknown_platform && row.platform == wanted
        }
    })
}

/// Draws the featured cover for the selected game.
///
/// The box is reserved at a fixed size for this frame whatever the artwork is
/// doing, so the title and the actions beneath it never move as a cover arrives,
/// fails, or turns out not to exist.
///
/// Allocated with [`egui::Sense::hover`] and painted directly: artwork is not a
/// control, so it takes no place in the Tab order and Mount stays the first thing
/// a keyboard reaches.
pub(crate) fn show_featured_cover(
    ui: &mut egui::Ui,
    box_size: egui::Vec2,
    cover: Option<&crate::gamer_artwork::CoverSlot>,
    placeholder: GameRowArtworkPaint<'_>,
    artwork_cache: &mut PlatformArtworkCache,
    artwork_directory: Option<&Path>,
) {
    let fallback = matches!(cover, Some(crate::gamer_artwork::CoverSlot::None(_)))
        .then_some("No cover available");
    widgets::media_frame(ui, box_size, fallback, |ui, rect| match cover {
        Some(crate::gamer_artwork::CoverSlot::Ready { texture, .. }) => {
            let drawn = crate::gamer_artwork::fit_within(box_size, texture.size_vec2());
            ui.painter().image(
                texture.id(),
                egui::Rect::from_center_size(rect.center(), drawn),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        _ => {
            let glyph = (box_size.y * 0.42).min(96.0);
            paint_game_row_artwork(
                ui,
                artwork_cache,
                artwork_directory,
                GameRowArtworkPaint {
                    center: rect.center() - egui::vec2(0.0, glyph * 0.16),
                    size: glyph,
                    ..placeholder
                },
            );
        }
    });
}

/// The featured panel's primary button: full width and taller than a secondary
/// one, so the current action reads as the thing to do from across a room.
///
/// Still `widgets::action_button`'s `Primary` fill and still an ordinary
/// `egui::Button`, so its enable/disable rules, focus behaviour and activation are
/// exactly what they were.
pub(crate) fn featured_primary_button(
    ui: &mut egui::Ui,
    label: &str,
    enabled: bool,
) -> egui::Response {
    let width = ui.available_width();
    ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(label).size(17.0).strong())
            .fill(theme::ACCENT)
            .min_size(egui::vec2(width, 42.0)),
    )
}

/// Gamer View styling for the same tracked executor state Advanced View
/// renders. Returns `true` only for an explicit enabled Play/retry click;
/// process creation remains exclusively in the caller's
/// `GamerViewAction::Play` handler and the shared launch executor.
fn featured_retroarch_launch_action(
    ui: &mut egui::Ui,
    launch_state: &mut launch_readiness_page::RetroArchLaunchState,
    request: &archivefs_core::launch::RetroArchLaunchRequest,
    enabled: bool,
) -> bool {
    use launch_readiness_page::RetroArchLaunchDisplay;

    let display = launch_state.display_for(request);
    launch_readiness_page::show_retroarch_launch_feedback(ui, &display);
    // Integration: the executor state machine, polling and running/closed/
    // error presentation are 934c9ec's; the plain beginner label is
    // 1c825e7's (`GAMER_PLAY_LABEL`).
    match display {
        RetroArchLaunchDisplay::Idle
        | RetroArchLaunchDisplay::Exited { .. }
        | RetroArchLaunchDisplay::Failed { .. } => {
            featured_primary_button(ui, GAMER_PLAY_LABEL, enabled).clicked()
        }
        RetroArchLaunchDisplay::Starting => {
            featured_primary_button(ui, "Starting…", false);
            false
        }
        RetroArchLaunchDisplay::Running { .. } => {
            featured_primary_button(ui, GAMER_PLAY_LABEL, false);
            false
        }
    }
}

fn featured_typed_launch_action(
    ui: &mut egui::Ui,
    request: &launch_readiness_page::TypedLaunchRequest,
    retroarch_launch_state: &mut launch_readiness_page::RetroArchLaunchState,
    enabled: bool,
) -> bool {
    match request {
        launch_readiness_page::TypedLaunchRequest::RetroArch(request) => {
            featured_retroarch_launch_action(ui, retroarch_launch_state, request, enabled)
        }
        _ => {
            let label = format!("{GAMER_PLAY_LABEL} with {}", request.adapter_name());
            featured_primary_button(ui, &label, enabled).clicked()
        }
    }
}

/// One line of the featured panel's metadata block.
pub(crate) fn featured_meta_line(ui: &mut egui::Ui, text: String, strong: bool) {
    let text = egui::RichText::new(text).size(if strong { 16.0 } else { 14.0 });
    ui.label(if strong {
        text.color(theme::PRIMARY_TEXT)
    } else {
        text.color(theme::muted(ui))
    });
}

/// Merges `record.metadata`'s own fields over a RomM enrichment answer,
/// field by field, `record.metadata` always winning when both have a
/// value.
///
/// This is the one place the merge happens, and the direction is
/// deliberate: `record.metadata` is whatever the app's own
/// `FilenameMetadataProvider` (or, in the future, a manual edit) already
/// decided, and enrichment only ever fills a *gap*, never replaces an
/// existing answer - the architecture rule this whole milestone exists
/// under ("metadata enriches the game record; metadata does not become
/// preservation identity authority") applies just as much to one
/// enrichment-only field quietly overwriting another as it would to
/// enrichment touching identity.
/// The featured panel's platform line: platform, archive format, and -
/// when known - release year on one line together, so a game's year sits
/// naturally beside what it *is* rather than getting its own "Released
/// 1998" line lower in the panel.
fn featured_platform_line(platform: &str, format_name: &str, year: Option<u16>) -> String {
    match year {
        Some(year) => format!("{platform} \u{b7} {format_name} \u{b7} {year}"),
        None => format!("{platform} \u{b7} {format_name}"),
    }
}

struct GamerMetadataView<'a> {
    synopsis: Option<&'a str>,
    genre: Option<&'a str>,
    players: Option<&'a str>,
    rating: Option<u8>,
    release_year: Option<u16>,
}

impl<'a> GamerMetadataView<'a> {
    fn merge(
        record_metadata: &'a archivefs_core::ArchiveMetadata,
        enrichment: Option<&'a crate::game_metadata::GameMetadataResult>,
    ) -> Self {
        let found = match enrichment {
            Some(crate::game_metadata::GameMetadataResult::Found(metadata)) => Some(metadata),
            _ => None,
        };
        Self {
            synopsis: record_metadata
                .synopsis
                .as_deref()
                .or_else(|| found.and_then(|m| m.synopsis.as_deref())),
            genre: record_metadata
                .genre
                .as_deref()
                .or_else(|| found.and_then(|m| m.genre.as_deref())),
            players: record_metadata
                .players
                .as_deref()
                .or_else(|| found.and_then(|m| m.players.as_deref())),
            rating: record_metadata
                .rating
                .or_else(|| found.and_then(|m| m.rating)),
            release_year: record_metadata
                .release_year
                .or_else(|| found.and_then(|m| m.release_year)),
        }
    }

    fn is_empty(&self) -> bool {
        self.synopsis.is_none()
            && self.genre.is_none()
            && self.players.is_none()
            && self.rating.is_none()
            && self.release_year.is_none()
    }
}

/// Splits a joined genre string back into its individual labels for chip
/// display. `enrichment_metadata` (core) is the one place that joins a
/// provider's genre list with `", "`; this is its exact inverse, kept
/// local to presentation rather than changing `ArchiveMetadata::genre`'s
/// shape (a `String`, used by other display sites too) for one panel's
/// chip layout. Genre names have not been observed to contain a literal
/// `", "`, so this round-trips cleanly in practice; a name that somehow
/// did would simply render as one chip spanning two labels' worth of
/// text, not lost or malformed data.
fn split_genre_list(genre: &str) -> Vec<&str> {
    genre.split(", ").filter(|part| !part.is_empty()).collect()
}

/// "1 player", "4 players", "1-4 players" - never the placeholder-looking
/// "player(s)". A range (RomM's own common shape, e.g. "1-2") is always
/// worded as plural, since a range whose top end is 1 does not occur in
/// practice and "1-1 player" would read strangely if it did.
fn format_players(players: &str) -> String {
    match players.trim().parse::<u32>() {
        Ok(1) => "1 player".to_string(),
        _ => format!("{} players", players.trim()),
    }
}

/// A community/critic rating, 0-100, as a percentage - the same number
/// [`archivefs_core::ArchiveMetadata::rating`] already holds, just read as
/// the percentage it already is rather than a test-style "N/100" score.
/// The Details screen keeps the `/100` form alongside provenance, so the
/// original normalised value stays available there.
fn format_rating(rating: u8) -> String {
    format!("{rating}% rating")
}

/// Characters shown before a long synopsis is truncated with "Show more" -
/// long enough to read a real paragraph's opening without scrolling, short
/// enough that a short synopsis (a couple of sentences) usually needs no
/// truncation at all. Word-boundary aware, so a truncation never lands
/// mid-word.
const SYNOPSIS_PREVIEW_CHARS: usize = 220;

/// Bounded height for an *expanded* synopsis - generous enough that a real
/// provider synopsis (the longest observed in live validation was ~1,200
/// characters) wraps naturally with no inner scrollbar at a typical panel
/// width, while still capping a pathological one so it cannot push the
/// primary action button off screen; a scrollbar only ever appears in that
/// unusual case, never for an ordinary synopsis.
const SYNOPSIS_EXPANDED_MAX_HEIGHT: f32 = 260.0;

/// The synopsis block: full text with natural wrapping when it is short
/// enough to need no truncation, otherwise a preview and a "Show more"
/// toggle rather than a permanently visible tiny scrollbar. Expand/collapse
/// state is keyed by the synopsis text itself, so it starts collapsed again
/// automatically for whichever game is selected next - nothing has to
/// track "which game this state belongs to" separately.
fn show_synopsis(ui: &mut egui::Ui, synopsis: &str) {
    let truncated = synopsis
        .char_indices()
        .nth(SYNOPSIS_PREVIEW_CHARS)
        .is_some();
    if !truncated {
        ui.label(
            egui::RichText::new(synopsis)
                .size(13.0)
                .color(theme::muted(ui)),
        );
        return;
    }
    let expand_id = egui::Id::new("gamer_featured_synopsis_expanded").with(synopsis);
    let mut expanded = ui.data(|data| data.get_temp::<bool>(expand_id).unwrap_or(false));
    if expanded {
        egui::ScrollArea::vertical()
            .id_salt(("gamer_featured_synopsis_scroll", synopsis))
            .max_height(SYNOPSIS_EXPANDED_MAX_HEIGHT)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(synopsis)
                        .size(13.0)
                        .color(theme::muted(ui)),
                );
            });
    } else {
        let boundary = synopsis[..synopsis
            .char_indices()
            .nth(SYNOPSIS_PREVIEW_CHARS)
            .map_or(synopsis.len(), |(i, _)| i)]
            .rfind(char::is_whitespace)
            .unwrap_or(SYNOPSIS_PREVIEW_CHARS.min(synopsis.len()));
        ui.label(
            egui::RichText::new(format!("{}\u{2026}", synopsis[..boundary].trim_end()))
                .size(13.0)
                .color(theme::muted(ui)),
        );
    }
    if widgets::action_button(
        ui,
        if expanded { "Show less" } else { "Show more" },
        widgets::ActionStyle::Secondary,
        true,
    )
    .clicked()
    {
        expanded = !expanded;
        ui.data_mut(|data| data.insert_temp(expand_id, expanded));
    }
}

/// The featured panel's optional enrichment block: a bounded/expandable
/// synopsis, genre chips, and compact players/rating lines - whatever of
/// these the merged view actually has. Renders nothing at all - not even a
/// heading - when nothing was found, per this milestone's explicit rule
/// that missing fields simply disappear rather than showing rows of
/// "Unknown". Release year is shown by the caller, alongside platform, not
/// here - see the featured panel's platform/format/year line.
fn show_gamer_metadata_enrichment(ui: &mut egui::Ui, view: &GamerMetadataView<'_>) {
    if view.is_empty() {
        return;
    }
    if let Some(synopsis) = view.synopsis {
        ui.add_space(4.0);
        show_synopsis(ui, synopsis);
        ui.add_space(4.0);
    }
    if let Some(genre) = view.genre {
        widgets::info_chip_row(ui, &split_genre_list(genre));
        ui.add_space(2.0);
    }
    if let Some(players) = view.players {
        featured_meta_line(ui, format_players(players), false);
    }
    if let Some(rating) = view.rating {
        featured_meta_line(ui, format_rating(rating), false);
    }
}

pub(crate) struct GamerViewViewState<'a> {
    pub(crate) filter: &'a str,
    pub(crate) library_filters: &'a mut LibraryRowFilters,
    pub(crate) archive_context: &'a mut ArchiveContext,
    pub(crate) screen: &'a mut GamerViewScreen,
    pub(crate) busy: bool,
    pub(crate) block_reason: Option<&'static str>,
    pub(crate) cleanup_after_unmount: bool,
    pub(crate) cheat_workflow: Option<&'a CheatWorkflowState>,
    pub(crate) feedback: Option<&'a ActionFeedback>,
    pub(crate) scan_review_available: bool,
    pub(crate) artwork_directory: Option<&'a Path>,
    pub(crate) artwork_cache: &'a mut PlatformArtworkCache,
    /// RomM covers already answered for, and the scheduling that decides what
    /// to ask about next.
    pub(crate) covers: &'a mut crate::gamer_artwork::GamerCoverCache,
    /// Filled with the records this frame wants covers for. The view itself
    /// never sends anything: it reports what the visible window needs and the
    /// caller hands that to the worker, which keeps this function free of
    /// threads and testable without one.
    pub(crate) cover_requests: &'a mut Vec<crate::gamer_artwork::CoverJob>,
    /// Enrichment (synopsis/genre/players/rating/release year) already
    /// resolved for the currently focused game, if any - `None` until the
    /// caller's worker has answered, distinct from
    /// [`crate::game_metadata::GameMetadataResult::NotFound`]/
    /// [`crate::game_metadata::GameMetadataResult::Unavailable`], both of
    /// which are real (already-answered) results this view still renders
    /// (as "no game information available" wording), never network calls.
    pub(crate) game_metadata: Option<&'a crate::game_metadata::GameMetadataResult>,
    pub(crate) identity_status: Option<GamerIdentityStatus>,
    pub(crate) prepared_member: bool,
    pub(crate) member_choices: Option<&'a [archivefs_core::PreparedMemberCandidate]>,
    pub(crate) preparation_message: Option<&'a str>,
    pub(crate) play_action: &'a launch_readiness_page::GamerPlayAction,
    pub(crate) retroarch_launch_state: &'a mut launch_readiness_page::RetroArchLaunchState,
    pub(crate) dolphin_launch_state: &'a mut launch_readiness_page::DolphinLaunchState,
    pub(crate) pcsx2_launch_state: &'a mut launch_readiness_page::Pcsx2LaunchState,
    pub(crate) standalone_launch_state: &'a mut launch_readiness_page::StandaloneLaunchState,
}

/// The read-only Details screen (finding #2): identity/platform/metadata
/// only - no mount/unmount/platform-assignment controls here. Those live
/// exclusively on the action panel (Stage 3), so there is exactly one
/// place a beginner ever clicks to change a game's state, not two
/// slightly-different copies of the same buttons.
pub(crate) fn show_gamer_details_panel(
    ui: &mut egui::Ui,
    record: &ArchiveRecord,
    enrichment: Option<&crate::game_metadata::GameMetadataResult>,
    identity_status: Option<GamerIdentityStatus>,
) -> Option<GamerViewAction> {
    let view = GamerMetadataView::merge(&record.metadata, enrichment);
    widgets::section_header(ui, "Details", None);
    egui::Grid::new("gamer_details_grid")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            detail_row(
                ui,
                "Platform",
                record
                    .metadata
                    .platform
                    .as_deref()
                    .or(record.identity.platform.as_deref())
                    .unwrap_or("Unknown"),
            );
            if let Some(status) = identity_status {
                detail_row(ui, "Identity", status.label());
            }
            detail_row(
                ui,
                "Format",
                archive_kind_name(record.mount_plan.archive.kind),
            );
            detail_row(ui, "Size", &format_size(record.identity.size_bytes));
            optional_detail_row(ui, "Title", record.metadata.title.as_deref());
            optional_detail_row(ui, "Region", record.metadata.region.as_deref());
            optional_detail_row(ui, "Version", record.metadata.version.as_deref());
            optional_detail_row(ui, "Disc", record.metadata.disc.as_deref());
            optional_detail_row(ui, "Publisher", record.metadata.publisher.as_deref());
            optional_detail_row(ui, "Developer", record.metadata.developer.as_deref());
            optional_detail_row(ui, "Genre", view.genre);
            optional_detail_row(
                ui,
                "Release year",
                view.release_year.map(|year| year.to_string()).as_deref(),
            );
            optional_detail_row(ui, "Players", view.players);
            optional_detail_row(
                ui,
                "Rating",
                view.rating.map(|rating| format!("{rating}/100")).as_deref(),
            );
        });
    if let Some(synopsis) = view.synopsis {
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Synopsis").strong());
        egui::ScrollArea::vertical()
            .id_salt("gamer_details_synopsis")
            .max_height(160.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.label(egui::RichText::new(synopsis).color(theme::muted(ui)));
            });
    }
    show_game_information_provenance(ui, enrichment)
}

/// Game-information (enrichment) provenance and the "Refresh game
/// information" action - kept entirely separate from the identity/mount
/// technical details above, since this data was never part of what
/// established what this game *is*, only what is known *about* it.
fn show_game_information_provenance(
    ui: &mut egui::Ui,
    enrichment: Option<&crate::game_metadata::GameMetadataResult>,
) -> Option<GamerViewAction> {
    use crate::game_metadata::GameMetadataResult;

    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(ui, "Game information", None);
    match enrichment {
        Some(GameMetadataResult::Found(metadata)) => {
            widgets::technical_details(ui, "gamer_game_information_details", |ui| {
                ui.label(format!(
                    "Source: {}",
                    metadata.source.as_deref().unwrap_or("Unknown provider")
                ));
                ui.label("Read from the locally cached catalogue - not fetched while browsing.");
            });
        }
        Some(GameMetadataResult::NotFound) => {
            ui.label(
                egui::RichText::new("No extra game information is available for this game.")
                    .color(theme::muted(ui)),
            );
        }
        Some(GameMetadataResult::Unavailable) | None => {
            widgets::failure_summary(
                ui,
                "gamer_game_information_unavailable",
                "We couldn't load game information right now",
                Some("The game itself is unaffected - mounting and playing work normally."),
                "No configured metadata source could be read (not configured, never synced, \
                 or its cached catalogue could not be opened).",
            );
        }
    }
    if widgets::action_button(
        ui,
        "Update game information",
        widgets::ActionStyle::Secondary,
        true,
    )
    .on_hover_text(
        "Re-reads game information already cached from a previous sync. To fetch new \
         information from the configured source, use Sync in Sources \u{2192} RomM.",
    )
    .clicked()
    {
        return Some(GamerViewAction::RefreshGameInformation);
    }
    None
}

/// Gamer View's one primary screen (Stage 2/3): search + platform chips +
/// game list on the left, the selected-game action panel on the right -
/// or, when `screen` is `Details`, the read-only Details view instead.
/// Returns `None` while data is still loading (the caller shows its own
/// loading state via `data: None`).
pub(crate) fn show_gamer_view(
    ui: &mut egui::Ui,
    data: Option<&LoadedData>,
    view_state: GamerViewViewState<'_>,
) -> Option<GamerViewAction> {
    let GamerViewViewState {
        filter,
        library_filters,
        archive_context,
        screen,
        busy,
        block_reason,
        cleanup_after_unmount,
        cheat_workflow,
        feedback,
        scan_review_available,
        artwork_directory,
        artwork_cache,
        covers,
        cover_requests,
        game_metadata,
        identity_status,
        prepared_member,
        member_choices,
        preparation_message,
        play_action,
        retroarch_launch_state,
        dolphin_launch_state,
        pcsx2_launch_state,
        standalone_launch_state,
    } = view_state;
    let mut action = None;

    if retroarch_launch_state.poll() || retroarch_launch_state.is_active() {
        ui.ctx().request_repaint();
    }
    if dolphin_launch_state.poll() || dolphin_launch_state.is_active() {
        ui.ctx().request_repaint();
    }
    if pcsx2_launch_state.poll() || pcsx2_launch_state.is_active() {
        ui.ctx().request_repaint();
    }
    if standalone_launch_state.poll() || standalone_launch_state.is_active() {
        ui.ctx().request_repaint();
    }

    if let Some(feedback) = feedback {
        let color = if feedback.succeeded {
            egui::Color32::from_rgb(70, 170, 90)
        } else {
            ui.visuals().error_fg_color
        };
        let message = if feedback.succeeded {
            std::borrow::Cow::Borrowed(feedback.message.as_str())
        } else {
            std::borrow::Cow::Owned(gamer_view_failure_message(&feedback.message))
        };
        ui.colored_label(color, message.as_ref());
        if feedback.succeeded
            && scan_review_available
            && widgets::action_button(ui, "Review", widgets::ActionStyle::Quiet, true).clicked()
        {
            action = Some(GamerViewAction::ReviewScan);
        }
        if let Some(more_information) = &feedback.more_information {
            widgets::technical_details(
                ui,
                ("gamer_action_feedback_details", more_information.as_str()),
                |ui| {
                    ui.label(more_information);
                },
            );
        }
        ui.add_space(4.0);
    }

    let Some(data) = data else {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Loading your games...");
        });
        return None;
    };

    if *screen == GamerViewScreen::Details {
        if ui.button("\u{2190} Back to games").clicked() {
            *screen = GamerViewScreen::GameList;
        }
        ui.add_space(theme::SECTION_GAP);
        let details_action =
            match selected_record(&data.records, archive_context.focused.as_deref()) {
                Some(record) => {
                    show_gamer_details_panel(ui, record, game_metadata, identity_status)
                }
                None => {
                    ui.label("Select a game to view its details.");
                    None
                }
            };
        return details_action;
    }

    let search_text = filter.to_lowercase();
    // One authoritative snapshot for this frame - see
    // `GamerLibrarySnapshot`. Everything below (shelf counts, the "All"
    // count, the game list, and the empty-state wording) reads from it,
    // so the shelf can never advertise a count the list cannot produce.
    let snapshot = GamerLibrarySnapshot::build(
        &data.rows,
        library_filters.platform.as_deref(),
        &search_text,
    );
    // A platform that is no longer in the snapshot (library reloaded, a
    // source removed, or a canonical id written by another page - see
    // `open_cheat_archive_picker`) would otherwise leave Gamer View stuck
    // on an empty list with no card highlighted and no way back except a
    // restart. Falling back to All is the only truthful resolution, and it
    // happens before the shelf is drawn so this frame already shows it.
    if snapshot.selection_is_stale {
        library_filters.platform = None;
        archive_context.clear_selection();
    }
    let visible = &snapshot.visible;
    let platform_counts = &snapshot.platform_counts;
    // Captured once for this frame so the list and the featured panel agree about
    // what is selected, and so a cover is asked for by the record's own path rather
    // than by whichever row happens to be drawn at its position.
    let selected_path = archive_context.focused.clone();

    // Once a library exists, keep its two recurring maintenance actions in
    // the simple view. Both dispatch the same source actions as Advanced View;
    // this layer adds no filesystem walking or scan implementation of its own.
    if !data.rows.is_empty() {
        ui.horizontal_wrapped(|ui| {
            if widgets::action_button(
                ui,
                "Add another game folder",
                widgets::ActionStyle::Secondary,
                !busy,
            )
            .on_hover_text("Choose another folder for EmuWiz to look through.")
            .clicked()
                && let Some(folder) = rfd::FileDialog::new()
                    .set_title("Choose another games folder")
                    .pick_folder()
            {
                action = Some(GamerViewAction::AddGamesFolder(folder));
            }
            if widgets::action_button(ui, "Scan for new games", widgets::ActionStyle::Quiet, !busy)
                .on_hover_text("Look through all your game folders again.")
                .clicked()
            {
                action = Some(GamerViewAction::ScanForNewGames);
            }
        });
        ui.add_space(theme::SECTION_GAP);
    }

    // The visual platform picker (milestone: "Gamer View Visual Platform
    // Picker and Library Layout Polish"): a single-row, horizontally
    // scrollable shelf, never wrapping onto multiple lines regardless of
    // how many platforms are present - "must not consume most of the
    // vertical height" is true unconditionally, not just for a typical
    // library. A fixed shelf height also means `available_height` below
    // is captured after a *known* amount of space is consumed, not a
    // wrapping row count that varies with window width.
    let platform_card_width = gamer_platform_card_width(ui.available_width());
    let mut shelf_artwork = PlatformShelfArtwork {
        directory: artwork_directory,
        cache: artwork_cache,
    };

    // Built here rather than inside the shelf so that this function keeps
    // ownership of what a platform means, and the shelf only draws and
    // navigates. The order is the order a person sees: All, then each detected
    // platform, then Unknown last if any.
    let mut entries: Vec<ShelfEntry<'_>> = vec![ShelfEntry {
        asset_id: PlatformAssetCategory::Console.asset_id().to_owned(),
        label: "All",
        count: snapshot.candidates.len(),
        platform: None,
    }];
    for (platform, count) in &platform_counts.named {
        entries.push(ShelfEntry {
            asset_id: platform_asset_id(platform, false),
            label: platform.as_str(),
            count: *count,
            platform: Some(platform.as_str()),
        });
    }
    if platform_counts.unknown > 0 {
        entries.push(ShelfEntry {
            asset_id: PlatformAssetCategory::Unknown.asset_id().to_owned(),
            label: "Unknown",
            count: platform_counts.unknown,
            platform: Some("Unknown"),
        });
    }

    let shelf = show_gamer_platform_shelf(
        ui,
        &entries,
        library_filters.platform.as_deref(),
        platform_card_width,
        &mut shelf_artwork,
        PLATFORM_SHELF_HEIGHT,
    );
    if let Some(chosen) = shelf.chosen {
        library_filters.platform = chosen;
        // Consistent with Advanced View's Library platform strip
        // (docs/GUI_NAVIGATION_RESET_DESIGN.md mandatory risk #3): every
        // platform-selection change clears the current focus rather than
        // risking a stale selected-game panel for a game no longer in view.
        archive_context.clear_selection();
    }
    ui.add_space(theme::SECTION_GAP);

    // Captured once, here, after the chips above have consumed their
    // actual height - and given explicitly to both columns below via
    // `allocate_ui_with_layout`, the same technique `ui_layout::page`
    // itself uses to guarantee a child gets the full height it was
    // promised rather than an ambiguous inherited one (manual QA finding:
    // the list must use all remaining height, with no large empty area
    // left below it).
    let available_width = ui.available_width();
    let available_height = ui.available_height();
    let list_width = (available_width * 0.6).max(280.0);
    let panel_width = (available_width - list_width - 24.0).max(220.0);

    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(list_width, available_height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                if visible.is_empty() {
                    // Distinguish *why* nothing is showing - a truthful,
                    // specific empty state rather than one generic
                    // message for every cause. Reachable now only when the
                    // snapshot genuinely holds no such row: the shelf
                    // counts these same candidates, so "no games match the
                    // selected platform" cannot contradict a non-zero card.
                    ui.add_space(theme::SECTION_GAP);
                    ui.weak(gamer_empty_list_guidance(
                        data.rows.is_empty(),
                        !search_text.is_empty(),
                        library_filters.platform.is_some(),
                    ));
                    // First-run only: no games at all, and no search/
                    // platform filter is hiding anything either - the
                    // "nothing here yet" state a brand-new install starts
                    // in, per Phase 3's audit finding that this state
                    // previously had no action at all, only this text.
                    // Never Sources/DAT/adapter/mount/identity/evidence/
                    // transaction vocabulary - a plain folder picker.
                    if gamer_view_shows_add_games_button(
                        data.rows.is_empty(),
                        search_text.is_empty(),
                        library_filters.platform.is_none(),
                    ) {
                        ui.add_space(theme::SECTION_GAP);
                        if widgets::action_button(
                            ui,
                            "Add games",
                            widgets::ActionStyle::Primary,
                            true,
                        )
                        .on_hover_text(
                            "Choose the folder where your games are - EmuWiz will look inside \
                             and add what it finds.",
                        )
                        .clicked()
                            && let Some(folder) = rfd::FileDialog::new()
                                .set_title("Choose your games folder")
                                .pick_folder()
                        {
                            action = Some(GamerViewAction::AddGamesFolder(folder));
                        }
                    }
                } else {
                    let row_height = (ui.spacing().interact_size.y * 2.4).max(64.0);
                    egui::ScrollArea::vertical()
                        .id_salt("gamer_view_game_list")
                        .max_height(ui.available_height())
                        .auto_shrink([false, false])
                        .show_rows(ui, row_height, visible.len(), |ui, row_range| {
                            // Only what is on screen, plus a small look-ahead, is
                            // ever asked about. `show_rows` hands us the drawn
                            // range, so this is bounded by the viewport rather than
                            // by the library: a 13,891-record catalogue and a
                            // 20-record one queue the same amount of work.
                            let wanted = crate::gamer_artwork::look_ahead_range(
                                row_range.clone(),
                                visible.len(),
                            );
                            let paths_for = |range: std::ops::Range<usize>| {
                                range
                                    .map(|position| data.rows[visible[position]].path.clone())
                                    .collect::<Vec<_>>()
                            };
                            let on_screen = paths_for(row_range.clone());
                            let ahead: Vec<PathBuf> = paths_for(wanted)
                                .into_iter()
                                .filter(|path| !on_screen.contains(path))
                                .collect();
                            // The selected game is asked for first, so the featured
                            // panel fills before the rows a person has not looked at
                            // yet. It takes at most one of the frame's slots, which
                            // is what stops a held-down arrow key from starving the
                            // list.
                            cover_requests.extend(covers.visible(
                                selected_path.as_deref(),
                                &on_screen,
                                &ahead,
                            ));

                            for visible_index in row_range {
                                let index = visible[visible_index];
                                let record = &data.records[index];
                                let row = &data.rows[index];
                                let selected =
                                    archive_context.focused.as_deref() == Some(row.path.as_path());
                                // Phase 5: an unresolved platform is as much
                                // "needs attention" as a blocked mount is -
                                // both get the same list-row word, and the
                                // selected-game panel explains which one
                                // applies (and, for the platform case, offers
                                // Review) once this exact row is opened.
                                let state_label = gamer_view_row_state_label(
                                    row.unknown_platform,
                                    record.mount_state,
                                );
                                // Looked up by the row's own path, which is the
                                // record's identity - `show_rows` reuses row
                                // positions as the list scrolls, and anything held
                                // per position would eventually be painted beside a
                                // different game.
                                let cover = covers.slot_for(row.path.as_path(), None).cloned();
                                let label = format!(
                                    "{} \u{2014} {} \u{b7} {state_label}",
                                    gamer_display_title(record),
                                    row.platform
                                );
                                // The row's visible text is unchanged; only the
                                // tooltip gains the reason, so a placeholder is
                                // explainable without spending a row on it.
                                let hover = match &cover {
                                    Some(crate::gamer_artwork::CoverSlot::None(reason)) => {
                                        format!("{label}\n{}", reason.describe())
                                    }
                                    _ => label.clone(),
                                };
                                // Stronger selected-row emphasis (manual QA
                                // finding): bold text plus an explicit
                                // selection-colored stroke drawn around
                                // `selectable_label`'s own rect, layered on
                                // top of - not replacing - its existing
                                // keyboard focus, Tab order, and
                                // Enter/Space activation (a plain
                                // `Sense::click()` label would lose all of
                                // that, which the accessibility
                                // requirements below depend on).
                                let response = ui
                                    .add(
                                        egui::Button::new("")
                                            .min_size(egui::vec2(ui.available_width(), row_height))
                                            .selected(selected),
                                    )
                                    .on_hover_text(hover);
                                let artwork_center = egui::pos2(
                                    response.rect.left() + 33.0,
                                    response.rect.center().y,
                                );
                                // The cover is drawn to *fit* the slot the platform
                                // icon already occupied, never to fill it. That
                                // keeps a 2:3 cover, a square icon and a glyph all
                                // inside the same box, so no row changes height as
                                // artwork arrives and none is ever stretched.
                                //
                                let drawn = match &cover {
                                    Some(crate::gamer_artwork::CoverSlot::Ready {
                                        texture,
                                        ..
                                    }) => {
                                        paint_cover_fitted(
                                            ui,
                                            texture,
                                            artwork_center,
                                            crate::gamer_artwork::COVER_BOX,
                                        );
                                        true
                                    }
                                    // Loading and "no cover" draw the same thing.
                                    // A spinner that becomes a picture is the same
                                    // visual jump this is meant to avoid, and the
                                    // placeholder is already a truthful, readable
                                    // image of the platform.
                                    _ => false,
                                };
                                if !drawn {
                                    let platform_asset =
                                        platform_asset_id(&row.platform, row.unknown_platform);
                                    let platform_fallback = platform_fallback_asset_id(
                                        &row.platform,
                                        row.unknown_platform,
                                    );
                                    paint_game_row_artwork(
                                        ui,
                                        artwork_cache,
                                        artwork_directory,
                                        GameRowArtworkPaint {
                                            center: artwork_center,
                                            size: crate::gamer_artwork::COVER_BOX,
                                            title: &gamer_display_title(record),
                                            platform_asset: &platform_asset,
                                            platform_fallback,
                                        },
                                    );
                                }
                                ui.painter().text(
                                    egui::pos2(
                                        response.rect.left() + 68.0,
                                        response.rect.center().y,
                                    ),
                                    egui::Align2::LEFT_CENTER,
                                    label,
                                    egui::FontId::proportional(14.0),
                                    if selected {
                                        ui.visuals().selection.stroke.color
                                    } else {
                                        ui.visuals().text_color()
                                    },
                                );
                                if selected {
                                    ui.painter().rect_stroke(
                                        response.rect,
                                        4.0,
                                        egui::Stroke::new(
                                            2.0_f32,
                                            ui.visuals().selection.stroke.color,
                                        ),
                                        egui::StrokeKind::Inside,
                                    );
                                }
                                if response.clicked() {
                                    archive_context.select_only(row.path.clone());
                                    *screen = GamerViewScreen::GameList;
                                }
                            }
                        });
                }
            },
        );

        ui.separator();

        ui.allocate_ui_with_layout(
            egui::vec2(panel_width, available_height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                widgets::section_header(ui, "Selected game", None);
                match selected_record(&data.records, archive_context.focused.as_deref()) {
                    None => {
                        // Clear, specific empty-state guidance (manual QA
                        // finding) rather than a bare label.
                        ui.add_space(theme::SECTION_GAP);
                        ui.weak("No game selected.");
                        ui.label("Choose a game from the list on the left to see what you can do with it.");
                    }
                    Some(record) => {
                        // The featured block lives in its own constrained column so
                        // the title, the status and Mount stay a cohesive unit
                        // rather than stretching the full width of the panel.
                        let content_width = panel_width.min(crate::gamer_artwork::GAMER_FEATURED_CONTENT_MAX_WIDTH);
                        let archive_path = record.mount_plan.archive.path.clone();
                        let platform = record
                            .metadata
                            .platform
                            .as_deref()
                            .or(record.identity.platform.as_deref())
                            .unwrap_or("Unknown");
                        let title = gamer_display_title(record);
                        let row = data.rows.iter().find(|row| row.path == archive_path);
                        let unknown_platform = row.is_some_and(|row| row.unknown_platform);

                        // How much height everything *below* the artwork actually
                        // took last frame: the title, the metadata, the separators
                        // and both rows of actions.
                        //
                        // Measured rather than estimated, and measured as one block
                        // rather than only the buttons. Its height depends on whether
                        // the title wrapped to a second line, whether the primary
                        // action carries a note, whether the secondary row wrapped
                        // and whether Undo is offered - guessing it puts Mount below
                        // the fold the moment any of those changes, which is exactly
                        // what a 1280x720 window did. Stored per panel and converged
                        // on the first frame.
                        let actions_id = ui.id().with("gamer_featured_below_height");
                        let measured = ui
                            .ctx()
                            .data(|data| data.get_temp::<f32>(actions_id))
                            .unwrap_or(crate::gamer_artwork::FEATURED_RESERVED_BELOW);
                        // Clamped against the physical viewport as well as the
                        // container's own figure. The Gamer View column reports the
                        // height it was allocated, which on a short window is more
                        // than is actually on screen - the list beside it scrolls, so
                        // it never notices, but this panel does not, and sizing the
                        // artwork from the larger number is what pushed the secondary
                        // actions off the bottom at 1280x720.
                        let to_screen_bottom =
                            (ui.ctx().screen_rect().bottom() - ui.cursor().top()).max(0.0);
                        let usable = ui.available_height().min(to_screen_bottom);
                        // The measurement covers the block itself; the gap the
                        // artwork adds beneath it, and a couple of pixels of rounding
                        // in the last row's spacing, come off here.
                        let for_artwork = (usable - measured - theme::SECTION_GAP - 12.0)
                            .max(if to_screen_bottom >= 680.0 {
                                crate::gamer_artwork::FEATURED_COVER_MIN_HEIGHT
                            } else {
                                0.0
                            });

                        widgets::hero_card(ui, |ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(content_width, ui.available_height()),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                // --- Featured artwork ---
                                //
                                // Reserved at a fixed size for this frame whatever
                                // the artwork is doing, so the title and the actions
                                // beneath it never move as a cover arrives, fails, or
                                // turns out not to exist. Sized from what the actions
                                // left, so the artwork is what shrinks on a short
                                // window - never the controls.
                                //
                                // Looked up by the selected record's own path, which
                                // is what a cover is keyed by, so the panel reads the
                                // *new* selection's slot the instant the selection
                                // changes and a late reply for the previous one has
                                // nothing here to draw into.
                                if let Some(box_size) = crate::gamer_artwork::featured_cover_box(
                                    content_width,
                                    for_artwork,
                                ) {
                                    let cover =
                                        covers.slot_for(archive_path.as_path(), None).cloned();
                                    let platform_asset =
                                        platform_asset_id(platform, unknown_platform);
                                    let platform_fallback =
                                        platform_fallback_asset_id(platform, unknown_platform);
                                    show_featured_cover(
                                        ui,
                                        box_size,
                                        cover.as_ref(),
                                        GameRowArtworkPaint {
                                            center: egui::Pos2::ZERO,
                                            size: 0.0,
                                            title: &title,
                                            platform_asset: &platform_asset,
                                            platform_fallback,
                                        },
                                        artwork_cache,
                                        artwork_directory,
                                    );
                                    ui.add_space(theme::SECTION_GAP);
                                }

                                // --- Identity, status and actions ---
                                //
                                // Measured as one block, so the artwork above can be
                                // sized from what everything below it really needs.
                                let below = ui.scope(|ui| {
                                    // The title is the strongest element; everything
                                    // under it is one quiet line each. Deliberately
                                    // not every field the Details screen holds - this
                                    // is what a person needs to know they picked the
                                    // right game, not a diagnostic dump.
                                    ui.label(
                                        egui::RichText::new(&title)
                                            .size(theme::DISPLAY_SIZE)
                                            .strong()
                                            .color(ui.visuals().strong_text_color()),
                                    );
                                    let metadata_view =
                                        GamerMetadataView::merge(&record.metadata, game_metadata);
                                    featured_meta_line(
                                        ui,
                                        featured_platform_line(
                                            platform,
                                            archive_kind_name(record.mount_plan.archive.kind),
                                            metadata_view.release_year,
                                        ),
                                        false,
                                    );
                                    show_gamer_metadata_enrichment(ui, &metadata_view);
                                    // One reconciled readiness state feeds both
                                    // the status word and the primary action
                                    // below, so the card can never say
                                    // "Ready to play" while the action is
                                    // blocked.
                                    let readiness = if record.is_mount_input() {
                                        gamer_archive_readiness(
                                            record.mount_state,
                                            prepared_member,
                                            play_action,
                                            member_choices,
                                        )
                                    } else {
                                        gamer_readiness(record.mount_state, play_action)
                                    };
                                    featured_meta_line(
                                        ui,
                                        gamer_readiness_short_label(&readiness).to_string(),
                                        true,
                                    );
                                    if let Some(row) = row
                                        && row.origin != RowOrigin::Live
                                    {
                                        // Only shown when it changes what the
                                        // buttons below can be trusted to do -
                                        // Gamer View's own plain wording, not
                                        // Advanced View's precise state name
                                        // (RowOrigin::label()).
                                        featured_meta_line(
                                            ui,
                                            row.origin.gamer_view_label().to_string(),
                                            false,
                                        );
                                    }
                                    // Phase 5 (docs/GUI_NAVIGATION_RESET_DESIGN.md
                                    // §4.1's fallback, never previously built): an
                                    // unresolved platform said only "Unknown" with
                                    // no explanation and no way to help EmuWiz
                                    // figure it out. Never says "identity",
                                    // "resolver", "evidence" - just what's true
                                    // (we're not sure) and one real next step
                                    // this architecture already supports (Review,
                                    // which lands on this exact game's existing
                                    // identity/evidence detail in Advanced View,
                                    // not a generic page).
                                    if let Some(row) = row
                                        && row.unknown_platform
                                    {
                                        ui.add_space(4.0);
                                        ui.colored_label(
                                            ui.visuals().warn_fg_color,
                                            "We couldn't tell which game system this is for.",
                                        );
                                        if widgets::action_button(
                                            ui,
                                            "Review",
                                            widgets::ActionStyle::Secondary,
                                            true,
                                        )
                                        .on_hover_text(
                                            "See what EmuWiz found for this game in Advanced \
                                             View, and help identify it if you can.",
                                        )
                                        .clicked()
                                        {
                                            action = Some(GamerViewAction::ReviewIdentity(
                                                archive_path.clone(),
                                            ));
                                        }
                                    }

                                    ui.add_space(theme::SECTION_GAP);
                                    ui.separator();
                                    ui.add_space(theme::SECTION_GAP);

                                    // Primary action: the one obvious, full-width,
                                    // visually prominent button for what this game
                                    // needs right now, and the first control a
                                    // keyboard reaches in this panel.
                                    match &readiness {
                                        GamerReadiness::Mount | GamerReadiness::Prepare => {
                                            ui.label(
                                                egui::RichText::new(
                                                    "Temporarily makes this archived game available. The original is unchanged.",
                                                )
                                                .color(theme::muted(ui)),
                                            );
                                            if let Some(message) = preparation_message {
                                                ui.colored_label(
                                                    ui.visuals().warn_fg_color,
                                                    message,
                                                );
                                            }
                                            if featured_primary_button(ui, "Prepare game", !busy)
                                                .clicked()
                                            {
                                                action = Some(GamerViewAction::Prepare(
                                                    archive_path.clone(),
                                                ));
                                            }
                                        }
                                        GamerReadiness::ChooseMember { candidates } => {
                                            ui.label("Choose the game file to play:");
                                            for candidate in *candidates {
                                                let label = format!(
                                                    "{} ({})",
                                                    candidate.member_name,
                                                    format_size(Some(candidate.size_bytes)),
                                                );
                                                if widgets::action_button(
                                                    ui,
                                                    &label,
                                                    widgets::ActionStyle::Secondary,
                                                    !busy,
                                                )
                                                .on_hover_text(&candidate.reason)
                                                .clicked()
                                                {
                                                    action = Some(
                                                        GamerViewAction::SelectArchiveMember(
                                                            archive_path.clone(),
                                                            candidate.member_name.clone(),
                                                        ),
                                                    );
                                                }
                                            }
                                        }
                                        GamerReadiness::Unmount => {
                                            if featured_primary_button(ui, "Unmount", !busy)
                                                .clicked()
                                            {
                                                action = Some(GamerViewAction::Operation(
                                                    OperationRequest {
                                                        action: ArchiveAction::Unmount,
                                                        archive_path: archive_path.clone(),
                                                        cleanup_after_unmount,
                                                    },
                                                ));
                                            }
                                        }
                                        GamerReadiness::Ready { request } => {
                                            if featured_typed_launch_action(
                                                ui,
                                                request,
                                                retroarch_launch_state,
                                                !busy,
                                            )
                                            {
                                                action = Some(GamerViewAction::Play(Box::new(
                                                    (**request).clone(),
                                                )));
                                            }
                                            widgets::technical_details(
                                                ui,
                                                "gamer-play-adapter",
                                                |ui| {
                                                    ui.label(format!(
                                                        "Uses the {} launch adapter.",
                                                        request.adapter_name()
                                                    ));
                                                },
                                            );
                                        }
                                        GamerReadiness::NeedsSetup { blocker } => {
                                            show_gamer_launch_blocker(ui, blocker);
                                            let (label, next_action) = match &blocker.kind {
                                                launch_readiness_page::GamerBlockerKind::UnknownSystem
                                                | launch_readiness_page::GamerBlockerKind::ConflictingIdentity => (
                                                    "Review game identity",
                                                    Some(GamerViewAction::ReviewIdentity(
                                                        archive_path.clone(),
                                                    )),
                                                ),
                                                launch_readiness_page::GamerBlockerKind::EmulatorNotInstalled
                                                | launch_readiness_page::GamerBlockerKind::EmulatorSetupIncomplete => (
                                                    "Open Emulator Setup",
                                                    blocker.emulator.as_deref().map_or_else(
                                                        || Some(GamerViewAction::CheckEmulators(
                                                            archive_path.clone(),
                                                        )),
                                                        |emulator| {
                                                            Some(GamerViewAction::OpenEmulatorSetup(
                                                                archive_path.clone(),
                                                                emulator_setup_focus(emulator),
                                                            ))
                                                        },
                                                    ),
                                                ),
                                                launch_readiness_page::GamerBlockerKind::EmulatorNotChecked => (
                                                    "Check Emulators",
                                                    Some(GamerViewAction::CheckEmulators(
                                                        archive_path.clone(),
                                                    )),
                                                ),
                                                launch_readiness_page::GamerBlockerKind::MultipleChoices => (
                                                    "Choose an emulator",
                                                    Some(GamerViewAction::OpenLaunchChoices(
                                                        archive_path.clone(),
                                                    )),
                                                ),
                                                launch_readiness_page::GamerBlockerKind::ContentNeedsPreparation
                                                | launch_readiness_page::GamerBlockerKind::LaunchPlanInvalid => (
                                                    "Open launch readiness",
                                                    Some(GamerViewAction::OpenLaunchChoices(
                                                        archive_path.clone(),
                                                    )),
                                                ),
                                                launch_readiness_page::GamerBlockerKind::NoSafeEmulator => (
                                                    "Check Emulators",
                                                    Some(GamerViewAction::CheckEmulators(
                                                        archive_path.clone(),
                                                    )),
                                                ),
                                                launch_readiness_page::GamerBlockerKind::CheckingGame => (
                                                    "Checking…",
                                                    None,
                                                ),
                                            };
                                            if let Some(next_action) = next_action
                                                && widgets::action_button(
                                                    ui,
                                                    label,
                                                    widgets::ActionStyle::Secondary,
                                                    !busy,
                                                )
                                                .clicked()
                                            {
                                                action = Some(next_action);
                                            }
                                        }
                                        GamerReadiness::NeedsAttention { reason } => {
                                            ui.colored_label(
                                                ui.visuals().warn_fg_color,
                                                reason.as_str(),
                                            );
                                        }
                                    }
                                    if let Some(reason) = block_reason {
                                        widgets::technical_details(
                                            ui,
                                            "gamer-operation-block",
                                            |ui| {
                                                ui.label(reason);
                                            },
                                        );
                                    }

                                    ui.add_space(theme::SECTION_GAP);
                                    ui.separator();
                                    ui.add_space(theme::SECTION_GAP);

                                    // Secondary actions: kept together rather than
                                    // scattered. One wrapping row while the panel is
                                    // wide enough for all three, a tidy full-width
                                    // stack once it is not.
                                    let stacked = content_width < GAMER_SECONDARY_ROW_MIN_WIDTH;
                                    let secondary = |ui: &mut egui::Ui, label: &str| {
                                        if stacked {
                                            let width = ui.available_width();
                                            ui.add(
                                                egui::Button::new(label)
                                                    .min_size(egui::vec2(width, 34.0)),
                                            )
                                        } else {
                                            widgets::action_button(
                                                ui,
                                                label,
                                                widgets::ActionStyle::Secondary,
                                                true,
                                            )
                                        }
                                    };
                                    let mut body = |ui: &mut egui::Ui| {
                                        if record.is_mount_input()
                                            && record.mount_state == MountState::Mounted
                                            && secondary(ui, "Unmount").clicked()
                                        {
                                            action = Some(GamerViewAction::Operation(
                                                OperationRequest {
                                                    action: ArchiveAction::Unmount,
                                                    archive_path: archive_path.clone(),
                                                    cleanup_after_unmount,
                                                },
                                            ));
                                        }
                                        if secondary(ui, "Cheats & Mods").clicked() {
                                            action = Some(GamerViewAction::OpenCheatsMods(
                                                archive_path.clone(),
                                            ));
                                        }
                                        if secondary(ui, "Details").clicked() {
                                            *screen = GamerViewScreen::Details;
                                        }
                                        let folder =
                                            archive_path.parent().filter(|folder| folder.is_dir());
                                        if let Some(folder) = folder
                                            && secondary(ui, gamer_copy_location_label()).clicked()
                                        {
                                            action = Some(GamerViewAction::CopyLocation(
                                                folder.display().to_string(),
                                            ));
                                        }
                                    };
                                    if stacked {
                                        body(ui);
                                    } else {
                                        ui.horizontal_wrapped(body);
                                    }

                                    if gamer_undo_available(
                                        cheat_workflow,
                                        Some(archive_path.as_path()),
                                    ) {
                                        ui.add_space(theme::SECTION_GAP);
                                        if widgets::action_button(
                                            ui,
                                            "Undo last change",
                                            widgets::ActionStyle::Quiet,
                                            true,
                                        )
                                        .clicked()
                                        {
                                            action = Some(GamerViewAction::Undo);
                                        }
                                    }
                                });

                                // Recorded for the next frame. A changed measurement
                                // asks for one more so the artwork settles
                                // immediately rather than on the next input.
                                let height = below.response.rect.height();
                                if (height - measured).abs() > 0.5 {
                                    ui.ctx()
                                        .data_mut(|data| data.insert_temp(actions_id, height));
                                    ui.ctx().request_repaint();
                                }
                            },
                        );
                        });
                    }
                }
            },
        );
    });

    action
}

/// Below this the featured panel's three secondary actions cannot share a row at
/// all, so they become a full-width stack rather than one button per line with a
/// ragged right edge. Above it they wrap, which degrades gracefully in between.
pub(crate) const GAMER_SECONDARY_ROW_MIN_WIDTH: f32 = 300.0;

#[cfg(test)]
mod game_metadata_enrichment_tests {
    use super::*;
    use crate::game_metadata::GameMetadataResult;

    fn rendered_text_contains(output: &egui::FullOutput, needle: &str) -> bool {
        fn shape_contains(shape: &egui::Shape, needle: &str) -> bool {
            match shape {
                egui::Shape::Text(text_shape) => text_shape.galley.text().contains(needle),
                egui::Shape::Vec(nested) => nested.iter().any(|s| shape_contains(s, needle)),
                _ => false,
            }
        }
        output
            .shapes
            .iter()
            .any(|clipped| shape_contains(&clipped.shape, needle))
    }

    #[test]
    fn clipboard_action_is_named_copy_folder_location() {
        assert_eq!(gamer_copy_location_label(), "Copy folder location");
        assert_ne!(gamer_copy_location_label(), "Open location");

        let action = GamerViewAction::CopyLocation("/games".to_string());
        assert!(matches!(action, GamerViewAction::CopyLocation(path) if path == "/games"));
    }

    #[test]
    fn mount_free_row_does_not_claim_launch_readiness() {
        let mount_only = gamer_primary_action(MountState::NotMountable);
        assert_eq!(gamer_primary_action_short_label(&mount_only), "Game found");

        let refusal = "Can't play yet: executable preflight rejected the selected core";
        let play_action = launch_readiness_page::GamerPlayAction::BlockedTyped(
            launch_readiness_page::GamerBlocker {
                kind: launch_readiness_page::GamerBlockerKind::LaunchPlanInvalid,
                emulator: None,
                detail: refusal.to_string(),
            },
        );
        let readiness = gamer_readiness(MountState::NotMountable, &play_action);
        assert!(matches!(
            readiness,
            GamerReadiness::NeedsSetup { blocker } if blocker.detail == refusal
        ));
    }

    #[test]
    fn archived_game_uses_prepare_game_wording_without_changing_operation_kind() {
        let action = gamer_primary_action(MountState::Pending);
        assert_eq!(gamer_primary_action_short_label(&action), "Prepare game");
        assert_eq!(
            gamer_readiness_short_label(&GamerReadiness::Mount),
            "Prepare game"
        );
        assert!(matches!(action, GamerPrimaryAction::Mount));
    }

    #[test]
    fn mounted_archive_shows_prepare_until_a_member_is_retained() {
        let play_action = launch_readiness_page::GamerPlayAction::BlockedTyped(
            launch_readiness_page::GamerBlocker {
                kind: launch_readiness_page::GamerBlockerKind::ContentNeedsPreparation,
                emulator: None,
                detail: "archive member has not been resolved".to_string(),
            },
        );
        let readiness = gamer_archive_readiness(MountState::Mounted, false, &play_action, None);
        assert!(matches!(readiness, GamerReadiness::Prepare));
        assert_eq!(gamer_readiness_short_label(&readiness), "Ready to prepare");
    }

    #[test]
    fn multiple_archive_members_show_a_chooser_instead_of_unmount_or_play() {
        let candidates = vec![archivefs_core::PreparedMemberCandidate {
            member_name: "disc1.iso".to_string(),
            size_bytes: 10,
            reason: "ISO media accepted for PlayStation 2".to_string(),
        }];
        let play_action = launch_readiness_page::GamerPlayAction::BlockedTyped(
            launch_readiness_page::GamerBlocker {
                kind: launch_readiness_page::GamerBlockerKind::ContentNeedsPreparation,
                emulator: None,
                detail: "archive member has not been resolved".to_string(),
            },
        );
        let readiness =
            gamer_archive_readiness(MountState::Mounted, false, &play_action, Some(&candidates));
        assert!(
            matches!(readiness, GamerReadiness::ChooseMember { candidates } if candidates.len() == 1)
        );
    }

    /// A ready RetroArch play action, standing in for the shared launch
    /// planner's `Launch` projection over an already-retained archive member.
    fn ready_retroarch_play_action() -> launch_readiness_page::GamerPlayAction {
        let request = archivefs_core::launch::RetroArchLaunchRequest {
            selected_content_path: PathBuf::from("/mnt/archive/disc1.iso"),
            expected_platform_id: "PlayStation 2".to_string(),
            expected_game_key: "retained-member".to_string(),
            profile: archivefs_core::emulator_environment::retroarch::ProfileRef {
                profile_kind: archivefs_core::emulator_environment::retroarch::ProfileKind::Native,
                scope: archivefs_core::emulator_environment::retroarch::ProfileScope::User,
            },
            core_stem: "pcsx2".to_string(),
        };
        launch_readiness_page::GamerPlayAction::Launch(Box::new(
            launch_readiness_page::TypedLaunchRequest::RetroArch(request),
        ))
    }

    #[test]
    fn mounted_archive_with_retained_member_and_ready_plan_shows_play() {
        let play_action = ready_retroarch_play_action();
        let readiness = gamer_archive_readiness(MountState::Mounted, true, &play_action, None);
        assert!(
            matches!(readiness, GamerReadiness::Ready { .. }),
            "a successfully retained member with a ready launch plan must reach Play, \
             not loop back to Prepare"
        );
        assert_eq!(gamer_readiness_short_label(&readiness), "Ready to play");
    }

    #[test]
    fn mounted_archive_with_retained_member_but_blocked_launch_routes_to_setup_not_prepare() {
        let refusal = "RetroArch is installed but its setup is incomplete";
        let play_action = launch_readiness_page::GamerPlayAction::BlockedTyped(
            launch_readiness_page::GamerBlocker {
                kind: launch_readiness_page::GamerBlockerKind::EmulatorSetupIncomplete,
                emulator: Some("RetroArch".to_string()),
                detail: refusal.to_string(),
            },
        );
        let readiness = gamer_archive_readiness(MountState::Mounted, true, &play_action, None);
        assert!(
            matches!(
                &readiness,
                GamerReadiness::NeedsSetup { blocker }
                    if blocker.kind
                        == launch_readiness_page::GamerBlockerKind::EmulatorSetupIncomplete
                        && blocker.detail == refusal
            ),
            "a retained member with a typed launch blocker must route to the blocker's \
             setup action, never back to Prepare"
        );
        assert_eq!(gamer_readiness_short_label(&readiness), "Needs setup");
    }

    #[test]
    fn retained_member_flag_is_ignored_until_the_archive_is_actually_mounted() {
        // `reconcile_archive_preparation` clears a stale Ready state, but even
        // if a `prepared` flag survived one frame it must never make an
        // unmounted archive look launch-ready.
        let play_action = ready_retroarch_play_action();
        let readiness = gamer_archive_readiness(MountState::Pending, true, &play_action, None);
        assert!(matches!(readiness, GamerReadiness::Prepare));
    }

    #[test]
    fn multiple_members_win_over_a_retained_member_and_ready_plan() {
        let candidates = vec![
            archivefs_core::PreparedMemberCandidate {
                member_name: "disc1.iso".to_string(),
                size_bytes: 10,
                reason: "ISO media accepted for PlayStation 2".to_string(),
            },
            archivefs_core::PreparedMemberCandidate {
                member_name: "disc2.iso".to_string(),
                size_bytes: 11,
                reason: "ISO media accepted for PlayStation 2".to_string(),
            },
        ];
        let play_action = ready_retroarch_play_action();
        let readiness =
            gamer_archive_readiness(MountState::Mounted, true, &play_action, Some(&candidates));
        assert!(
            matches!(readiness, GamerReadiness::ChooseMember { candidates } if candidates.len() == 2),
            "an unresolved multi-member choice is never auto-collapsed into Play"
        );
    }

    #[test]
    fn failed_preparation_keeps_prepare_and_surfaces_the_failure_message() {
        // `archive_preparation_view` reports a `Failed` state as
        // `prepared == false` plus a preparation message; readiness stays on
        // Prepare so the failure is shown above the retry button.
        let play_action = launch_readiness_page::GamerPlayAction::BlockedTyped(
            launch_readiness_page::GamerBlocker {
                kind: launch_readiness_page::GamerBlockerKind::ContentNeedsPreparation,
                emulator: None,
                detail: "archive member has not been resolved".to_string(),
            },
        );
        let readiness = gamer_archive_readiness(MountState::Mounted, false, &play_action, None);
        assert!(matches!(readiness, GamerReadiness::Prepare));
    }

    #[test]
    fn raw_launch_refusal_is_collapsed_by_default() {
        let refusal = "executable preflight rejected /cores/gambatte_libretro.so";
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_gamer_launch_blocker(
                    ui,
                    &launch_readiness_page::GamerBlocker {
                        kind: launch_readiness_page::GamerBlockerKind::LaunchPlanInvalid,
                        emulator: None,
                        detail: refusal.to_string(),
                    },
                );
            });
        });
        assert!(rendered_text_contains(&output, "Launch plan invalid"));
        assert!(rendered_text_contains(&output, "Technical details"));
        assert!(!rendered_text_contains(&output, refusal));
    }

    fn empty_archive_metadata() -> archivefs_core::ArchiveMetadata {
        archivefs_core::ArchiveMetadata {
            title: None,
            platform: None,
            region: None,
            languages: None,
            version: None,
            disc: None,
            publisher: None,
            developer: None,
            release_year: None,
            genre: None,
            notes: None,
            source: None,
            synopsis: None,
            players: None,
            rating: None,
        }
    }

    fn found(metadata: archivefs_core::ArchiveMetadata) -> GameMetadataResult {
        GameMetadataResult::Found(Box::new(metadata))
    }

    // --- GamerMetadataView::merge --------------------------------------

    #[test]
    fn merge_with_no_record_metadata_and_no_enrichment_is_empty() {
        let record_metadata = empty_archive_metadata();
        let view = GamerMetadataView::merge(&record_metadata, None);
        assert!(view.is_empty());
    }

    #[test]
    fn merge_prefers_the_records_own_metadata_over_romm_enrichment() {
        // The architecture rule under test: enrichment only ever fills a
        // gap, it never replaces something the record already had -
        // whether that came from filename parsing or, in the future, a
        // manual edit.
        let mut record_metadata = empty_archive_metadata();
        record_metadata.genre = Some("Strategy".to_string());
        let mut enrichment = empty_archive_metadata();
        enrichment.genre = Some("RPG".to_string());
        enrichment.players = Some("1-4".to_string());

        let enrichment_result = found(enrichment);
        let view = GamerMetadataView::merge(&record_metadata, Some(&enrichment_result));

        assert_eq!(
            view.genre,
            Some("Strategy"),
            "the record's own genre must win"
        );
        assert_eq!(
            view.players,
            Some("1-4"),
            "a field the record never had is still filled from enrichment"
        );
    }

    #[test]
    fn merge_ignores_enrichment_when_the_result_is_not_found() {
        let mut record_metadata = empty_archive_metadata();
        record_metadata.title = Some("Strong DAT Title".to_string());
        for result in [
            GameMetadataResult::NotFound,
            GameMetadataResult::Unavailable,
        ] {
            let view = GamerMetadataView::merge(&record_metadata, Some(&result));
            assert!(view.is_empty());
        }
    }

    // --- show_gamer_metadata_enrichment (Gamer View primary panel) -----

    fn view_of(metadata: &archivefs_core::ArchiveMetadata) -> GamerMetadataView<'_> {
        GamerMetadataView::merge(metadata, None)
    }

    #[test]
    fn nothing_is_rendered_when_no_field_was_found() {
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_gamer_metadata_enrichment(ui, &view_of(&empty_archive_metadata()));
            });
        });
        // No enrichment text of any kind, and specifically never a raw
        // "Unavailable"/"NotFound"-style internal label leaking into the
        // primary panel - missing fields simply disappear.
        assert!(!rendered_text_contains(&output, "Rating"));
        assert!(!rendered_text_contains(&output, "Unavailable"));
        assert!(!rendered_text_contains(&output, "NotFound"));
    }

    #[test]
    fn present_fields_render_and_absent_ones_do_not() {
        let mut metadata = empty_archive_metadata();
        metadata.synopsis = Some("A short adventure across five islands.".to_string());
        metadata.rating = Some(87);
        // Deliberately no genre and no players, to prove they are simply
        // omitted rather than shown as "Unknown".
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_gamer_metadata_enrichment(ui, &view_of(&metadata));
            });
        });
        assert!(rendered_text_contains(&output, "A short adventure"));
        assert!(rendered_text_contains(&output, "87% rating"));
        assert!(!rendered_text_contains(&output, "Unknown"));
    }

    #[test]
    fn a_sparse_record_with_only_one_field_shows_no_blank_rows_or_headings() {
        // Item 5's explicit rule: a single present field must still look
        // deliberate - no separators, headings, or "Unknown" placeholders
        // around it.
        let mut metadata = empty_archive_metadata();
        metadata.players = Some("2".to_string());
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_gamer_metadata_enrichment(ui, &view_of(&metadata));
            });
        });
        assert!(rendered_text_contains(&output, "2 players"));
        assert!(!rendered_text_contains(&output, "Unknown"));
        assert!(!rendered_text_contains(&output, "Genre"));
        assert!(!rendered_text_contains(&output, "Rating"));
        assert!(!rendered_text_contains(&output, "Synopsis"));
    }

    // --- players wording: item 8 ----------------------------------------

    #[test]
    fn a_single_player_is_worded_in_the_singular() {
        assert_eq!(format_players("1"), "1 player");
    }

    #[test]
    fn more_than_one_player_is_worded_in_the_plural() {
        assert_eq!(format_players("2"), "2 players");
        assert_eq!(format_players("4"), "4 players");
    }

    #[test]
    fn a_player_range_is_always_worded_in_the_plural() {
        assert_eq!(format_players("1-4"), "1-4 players");
        assert_eq!(format_players("1-2"), "1-2 players");
    }

    #[test]
    fn player_wording_never_uses_the_placeholder_shape() {
        for players in ["1", "2", "1-4"] {
            assert!(!format_players(players).contains("player(s)"));
        }
    }

    // --- rating wording: item 6 ------------------------------------------

    #[test]
    fn rating_is_presented_as_a_percentage_not_a_raw_score() {
        assert_eq!(format_rating(93), "93% rating");
        assert!(!format_rating(93).contains("/100"));
    }

    // --- genre chips: item 7 ----------------------------------------------

    #[test]
    fn a_single_genre_splits_to_one_chip() {
        assert_eq!(split_genre_list("Platformer"), vec!["Platformer"]);
    }

    #[test]
    fn several_genres_split_into_individual_chips() {
        assert_eq!(
            split_genre_list("Adventure, Puzzle, Platform, Shooter"),
            vec!["Adventure", "Puzzle", "Platform", "Shooter"]
        );
    }

    #[test]
    fn many_genres_render_as_wrapping_chips_not_one_long_run_on_line() {
        let mut metadata = empty_archive_metadata();
        metadata.genre =
            Some("Adventure, Hack and slash/Beat 'em up, Platform, Puzzle".to_string());
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_gamer_metadata_enrichment(ui, &view_of(&metadata));
            });
        });
        for genre in ["Adventure", "Platform", "Puzzle"] {
            assert!(rendered_text_contains(&output, genre));
        }
        // Each genre is its own chip/shape, never one comma-joined run.
        assert!(!rendered_text_contains(
            &output,
            "Adventure, Hack and slash/Beat 'em up, Platform, Puzzle"
        ));
    }

    // --- synopsis show more/less: item 3 ----------------------------------

    #[test]
    fn a_short_synopsis_needs_no_show_more_toggle() {
        let mut metadata = empty_archive_metadata();
        metadata.synopsis = Some("A short adventure across five islands.".to_string());
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_gamer_metadata_enrichment(ui, &view_of(&metadata));
            });
        });
        assert!(rendered_text_contains(&output, "A short adventure"));
        assert!(!rendered_text_contains(&output, "Show more"));
    }

    #[test]
    fn a_long_synopsis_is_truncated_with_a_show_more_toggle() {
        let mut metadata = empty_archive_metadata();
        // The real Ocarina of Time synopsis length observed live (1,096
        // characters) - long enough to need truncation.
        metadata.synopsis = Some("Adventure across time and space. ".repeat(35));
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_gamer_metadata_enrichment(ui, &view_of(&metadata));
            });
        });
        assert!(rendered_text_contains(&output, "Show more"));
        assert!(rendered_text_contains(&output, "\u{2026}"));
    }

    #[test]
    fn clicking_show_more_reveals_the_full_synopsis() {
        let mut metadata = empty_archive_metadata();
        metadata.synopsis = Some("Adventure across time and space. ".repeat(35));
        let ctx = egui::Context::default();
        let full_text = metadata.synopsis.clone().unwrap();
        let show_more_pos = {
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    show_gamer_metadata_enrichment(ui, &view_of(&metadata));
                });
            });
            text_rect_for_test(&output, "Show more")
                .expect("Show more must render")
                .center()
        };
        let _ = ctx.run(
            egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(show_more_pos),
                    egui::Event::PointerButton {
                        pos: show_more_pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::default(),
                    },
                    egui::Event::PointerButton {
                        pos: show_more_pos,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::default(),
                    },
                ],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    show_gamer_metadata_enrichment(ui, &view_of(&metadata));
                });
            },
        );
        // The click toggles state stored for next frame's render, exactly
        // as a real click-then-repaint does - a follow-up frame with no
        // new events is what shows the effect, the same way the click
        // itself would only visibly react on the frame after it lands.
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_gamer_metadata_enrichment(ui, &view_of(&metadata));
            });
        });
        assert!(rendered_text_contains(&output, &full_text));
        assert!(rendered_text_contains(&output, "Show less"));
    }

    #[test]
    fn a_very_long_synopsis_does_not_push_content_below_the_viewport() {
        // Bounded/scrollable rule: even an extreme synopsis, expanded, must
        // stay inside the reserved height rather than growing the panel -
        // the scroll fallback that protects against a pathological case.
        let mut metadata = empty_archive_metadata();
        metadata.synopsis = Some("Lorem ipsum dolor sit amet. ".repeat(200));
        let width = 460.0_f32;
        let height = 720.0_f32;
        let ctx = egui::Context::default();
        let run = |ctx: &egui::Context| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, height),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        show_gamer_metadata_enrichment(ui, &view_of(&metadata));
                        // Content that would appear "below" the enrichment
                        // block in the real panel - proves the synopsis
                        // didn't consume unbounded height and push this
                        // off-screen, whether collapsed or expanded.
                        ui.label("below the synopsis");
                    });
                },
            )
        };
        for output in [run(&ctx), run(&ctx)] {
            let rect =
                text_rect_for_test(&output, "below the synopsis").expect("the marker must render");
            assert!(
                rect.bottom() <= height,
                "content after a very long synopsis must stay within the viewport: {rect:?}"
            );
        }
    }

    #[test]
    fn a_fully_populated_real_record_leaves_room_for_content_below_it() {
        // Item 10: not just a long synopsis alone, but the whole enrichment
        // block together (synopsis, several genre chips, players, rating) -
        // real Ocarina of Time / Castlevania-shaped data - must still leave
        // the primary action visible at a normal window size.
        let mut metadata = empty_archive_metadata();
        metadata.synopsis = Some(
            "The Legend of Zelda: Ocarina of Time is the fifth main installment of \
             The Legend of Zelda series and the first to be released for the \
             Nintendo 64. It was one of the most highly anticipated games of its age."
                .to_string(),
        );
        metadata.genre =
            Some("Adventure, Hack and slash/Beat 'em up, Platform, Puzzle".to_string());
        metadata.players = Some("1-4".to_string());
        metadata.rating = Some(93);
        let width = 460.0_f32;
        let height = 720.0_f32;
        let ctx = egui::Context::default();
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, height),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    show_gamer_metadata_enrichment(ui, &view_of(&metadata));
                    ui.label("below the metadata block");
                });
            },
        );
        let rect = text_rect_for_test(&output, "below the metadata block")
            .expect("the marker must render");
        assert!(
            rect.bottom() <= height,
            "a fully populated real metadata block must still leave the primary action \
             visible: {rect:?}"
        );
    }

    // --- featured_platform_line: item 9 -----------------------------------

    #[test]
    fn the_platform_line_includes_the_year_when_known() {
        assert_eq!(
            featured_platform_line("Nintendo 64", "ZIP", Some(1998)),
            "Nintendo 64 \u{b7} ZIP \u{b7} 1998"
        );
    }

    #[test]
    fn the_platform_line_omits_the_year_when_unknown() {
        assert_eq!(
            featured_platform_line("Nintendo 64", "ZIP", None),
            "Nintendo 64 \u{b7} ZIP"
        );
    }

    // --- live-data validation (2026-08-22) ------------------------------
    //
    // The two cases below use fields captured from a real, live RomM 5.2.0
    // instance rather than invented values, to prove the render function
    // handles the shape real API data actually takes (multi-genre lists,
    // a rounded 0-100 rating, a real-length synopsis) and, separately, the
    // sparse case a real unmatched-in-RomM library entry produces (only one
    // field present, everything else absent).

    #[test]
    fn a_real_live_ocarina_of_time_lookup_renders_every_field_it_returned() {
        let mut metadata = empty_archive_metadata();
        metadata.genre = Some("Adventure, Puzzle".to_string());
        metadata.players = Some("1".to_string());
        metadata.rating = Some(93);
        metadata.release_year = Some(1998);
        metadata.synopsis = Some(
            "The Legend of Zelda: Ocarina of Time is the fifth main installment \
             of The Legend of Zelda series and the first to be released for the \
             Nintendo 64."
                .to_string(),
        );
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                featured_meta_line(
                    ui,
                    featured_platform_line("Nintendo 64", "ZIP", metadata.release_year),
                    false,
                );
                show_gamer_metadata_enrichment(ui, &view_of(&metadata));
            });
        });
        assert!(rendered_text_contains(&output, "1998"));
        assert!(rendered_text_contains(&output, "93% rating"));
        assert!(rendered_text_contains(&output, "Adventure"));
        assert!(rendered_text_contains(&output, "Puzzle"));
        assert!(rendered_text_contains(&output, "1 player"));
        assert!(rendered_text_contains(&output, "The Legend of Zelda"));
        assert!(!rendered_text_contains(&output, "Unknown"));
    }

    #[test]
    fn a_real_live_unmatched_library_entry_renders_only_its_one_present_field() {
        // "3 in 1 - Break Out + Centipede + Warlords # GBA.zip" (RomM id
        // 5541, live, 2026-08-22): RomM never matched this to a metadata
        // source, so every field is absent except `player_count: "1"`.
        let mut metadata = empty_archive_metadata();
        metadata.players = Some("1".to_string());
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_gamer_metadata_enrichment(ui, &view_of(&metadata));
            });
        });
        assert!(rendered_text_contains(&output, "1 player"));
        assert!(!rendered_text_contains(&output, "Rating"));
        assert!(!rendered_text_contains(&output, "Unknown"));
        assert!(!rendered_text_contains(&output, "null"));
    }

    fn text_rect_for_test(output: &egui::FullOutput, needle: &str) -> Option<egui::Rect> {
        fn walk(shape: &egui::Shape, needle: &str, out: &mut Option<egui::Rect>) {
            match shape {
                egui::Shape::Text(text) if text.galley.text() == needle && out.is_none() => {
                    *out = Some(text.visual_bounding_rect());
                }
                egui::Shape::Vec(nested) => nested.iter().for_each(|s| walk(s, needle, out)),
                _ => {}
            }
        }
        let mut found = None;
        for clipped in &output.shapes {
            walk(&clipped.shape, needle, &mut found);
        }
        found
    }

    // --- show_game_information_provenance (Details screen) -------------

    #[test]
    fn found_enrichment_offers_its_source_behind_technical_details() {
        // Provenance is exactly the kind of detail the milestone's own
        // rules keep out of the primary panel - it lives behind the same
        // shared progressive-disclosure control every other technical
        // section in this app uses, collapsed by default.
        let mut metadata = empty_archive_metadata();
        metadata.synopsis = Some("A story.".to_string());
        metadata.source = Some("RomM".to_string());
        let result = found(metadata);
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_game_information_provenance(ui, Some(&result));
            });
        });
        assert!(rendered_text_contains(&output, "Technical details"));
        assert!(
            !rendered_text_contains(&output, "Source: RomM"),
            "collapsed by default - the source string must not already be visible"
        );
    }

    #[test]
    fn unavailable_shows_human_wording_never_an_internal_label() {
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ =
                    show_game_information_provenance(ui, Some(&GameMetadataResult::Unavailable));
            });
        });
        assert!(rendered_text_contains(
            &output,
            "We couldn't load game information right now"
        ));
        assert!(rendered_text_contains(
            &output,
            "mounting and playing work normally"
        ));
        assert!(!rendered_text_contains(&output, "GameMetadataResult"));
        assert!(!rendered_text_contains(&output, "Unavailable"));
    }

    #[test]
    fn the_refresh_action_is_always_offered_regardless_of_state() {
        for state in [
            None,
            Some(GameMetadataResult::NotFound),
            Some(GameMetadataResult::Unavailable),
            Some(found(empty_archive_metadata())),
        ] {
            let ctx = egui::Context::default();
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = show_game_information_provenance(ui, state.as_ref());
                });
            });
            assert!(rendered_text_contains(&output, "Update game information"));
        }
    }
}
