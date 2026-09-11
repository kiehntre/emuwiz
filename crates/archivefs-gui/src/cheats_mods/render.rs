use crate::*;

pub(crate) fn dolphin_gamehacking_request_key(
    workflow: &CheatWorkflowState,
    generation: u64,
) -> Option<DolphinGameHackingRequestKey> {
    let report = ready_game_identity(workflow)?;
    let game_id = report.verified_dolphin_game_id()?.to_string();
    let is_wii = workflow.platform.as_deref() == Some("Wii");
    let revision = if is_wii {
        wii_identity_for_workflow(workflow)?.candidate_revision
    } else {
        report.verified_dolphin_revision()
    };
    Some(DolphinGameHackingRequestKey {
        archive_path: workflow.archive_path.clone(),
        platform: if is_wii { "Wii" } else { "GameCube" }.to_string(),
        game_id,
        revision,
        generation,
    })
}


pub(crate) fn gamecube_gamehacking_selection_for(
    cheats: &[GameHackingGameCubeCheat],
) -> GameCubeCheatSelection {
    GameCubeCheatSelection::from_cheats(cheats, &parse_dolphin_ini(""))
}


pub(crate) fn dolphin_game_from_wii_game(game: GameHackingWiiGame) -> GameHackingGameCubeGame {
    GameHackingGameCubeGame {
        game_id: game.game_id,
        title: game.title,
        system: game.system,
        region: game.region,
        dolphin_game_id: game.dolphin_game_id,
        revision: game.revision,
        hash: game.crc32,
        source_url: game.source_url,
    }
}


pub(crate) fn wii_game_from_dolphin_game(game: &GameHackingGameCubeGame) -> GameHackingWiiGame {
    GameHackingWiiGame {
        game_id: game.game_id,
        title: game.title.clone(),
        system: "Wii".to_string(),
        region: game.region.clone(),
        dolphin_game_id: game.dolphin_game_id.clone(),
        revision: game.revision,
        disc_number: None,
        crc32: game.hash.clone(),
        source_url: game.source_url.clone(),
    }
}


pub(crate) fn dolphin_candidate_from_wii(
    candidate: GameHackingWiiMatchCandidate,
) -> GameHackingGameCubeMatchCandidate {
    let strength = match candidate.strength {
        GameHackingWiiMatchStrength::ExactGameIdAndRevision => {
            GameHackingGameCubeMatchStrength::ExactGameIdAndRevision
        }
        GameHackingWiiMatchStrength::ExactGameIdAndRegion => {
            GameHackingGameCubeMatchStrength::ExactGameIdAndRegion
        }
        GameHackingWiiMatchStrength::ExactGameId
        | GameHackingWiiMatchStrength::ExactGameIdRevisionUnverified => {
            GameHackingGameCubeMatchStrength::ExactGameId
        }
    };
    GameHackingGameCubeMatchCandidate {
        game: dolphin_game_from_wii_game(candidate.game),
        strength,
        requires_user_confirmation: candidate.requires_user_confirmation,
    }
}


pub(crate) fn dolphin_cheat_from_wii(cheat: GameHackingWiiCheat) -> GameHackingGameCubeCheat {
    let code_format = if !cheat.safety.installable() {
        GameCubeCodeFormat::Unsupported
    } else {
        match cheat.code_format {
            WiiCodeFormat::ActionReplay => GameCubeCodeFormat::ActionReplay,
            WiiCodeFormat::Gecko => GameCubeCodeFormat::Gecko,
            WiiCodeFormat::RawUnknown => GameCubeCodeFormat::RawUnknown,
            WiiCodeFormat::Unsupported => GameCubeCodeFormat::Unsupported,
        }
    };
    let mut notes = cheat.description.into_iter().collect::<Vec<_>>();
    if !cheat.safety.installable() {
        notes.push(format!("Preview only: {}.", cheat.safety.reason()));
    }
    notes.extend(cheat.safety_warnings);
    GameHackingGameCubeCheat {
        id: cheat.id,
        name: cheat.name,
        author: cheat.author,
        description: (!notes.is_empty()).then(|| notes.join(" ")),
        code_format,
        code_lines: cheat.code_lines,
        source_game_id: cheat.source_game_id,
        source_url: cheat.source_url,
    }
}


pub(crate) fn wii_match_state(
    matched: GameHackingWiiMatch,
    cheats: Vec<GameHackingWiiCheat>,
    cached_fallback: bool,
) -> GameCubeGameHackingState {
    let status = match matched.status {
        GameHackingWiiMatchStatus::Matched => GameHackingGameCubeMatchStatus::Matched,
        GameHackingWiiMatchStatus::Candidates => GameHackingGameCubeMatchStatus::Candidates,
        GameHackingWiiMatchStatus::NoMatch => GameHackingGameCubeMatchStatus::NoMatch,
        GameHackingWiiMatchStatus::IdentityIncomplete => {
            GameHackingGameCubeMatchStatus::IdentityIncomplete
        }
    };
    let cheats = cheats
        .into_iter()
        .map(dolphin_cheat_from_wii)
        .collect::<Vec<_>>();
    GameCubeGameHackingState {
        status,
        detail: matched.detail,
        game: matched.game.map(dolphin_game_from_wii_game),
        match_candidates: matched
            .candidates
            .into_iter()
            .map(dolphin_candidate_from_wii)
            .collect(),
        selection: gamecube_gamehacking_selection_for(&cheats),
        cheats,
        cached_fallback,
    }
}


pub(crate) fn show_cheat_play_target_warning(
    ui: &mut egui::Ui,
    cheat_target: CheatEmulatorAdapter,
    play_target: Option<&'static str>,
) {
    let Some(cheat_target) = cheat_target.display_name() else {
        return;
    };
    let Some(play_target) = play_target else {
        return;
    };
    if cheat_target == play_target {
        return;
    }
    widgets::banner(
        ui,
        "Cheat target differs from Play",
        &format!(
            "These cheats target {cheat_target}, but this game is set to play with {play_target}. These cheats will not affect that launch."
        ),
        widgets::StatusTone::Warning,
    );
}


pub(crate) fn show_cheat_activation_status(
    ui: &mut egui::Ui,
    emulator: &str,
    readiness: CheatActivationReadiness,
) {
    let is_xenia = emulator == "Xenia";
    let (label, tone, detail) = match readiness {
        CheatActivationReadiness::Enabled => (
            if is_xenia {
                "Patch activation: Enabled"
            } else {
                "Cheat activation: Enabled"
            },
            widgets::StatusTone::Success,
            None,
        ),
        CheatActivationReadiness::Disabled => (
            if is_xenia {
                "Patch activation: Disabled"
            } else {
                "Cheat activation: Disabled"
            },
            widgets::StatusTone::Warning,
            Some(format!(
                "Turn on Enable Cheats in {emulator} before playing."
            )),
        ),
        CheatActivationReadiness::Unknown => (
            if is_xenia {
                "Patch activation: Not confirmed"
            } else {
                "Cheat activation: Not confirmed"
            },
            widgets::StatusTone::Warning,
            Some(if is_xenia {
                "Patch files can be installed here, but EmuWiz does not currently confirm Xenia's patch activation state.".to_string()
            } else {
                format!(
                    "EmuWiz can install the cheat file, but cannot confirm {emulator} will activate it automatically."
                )
            }),
        ),
    };
    widgets::status_badge(ui, label, tone);
    if let Some(detail) = detail {
        ui.label(detail);
    }
}


pub(crate) fn cheat_archive_change_requires_confirmation(
    workflow: Option<&CheatWorkflowState>,
    candidate: &Path,
) -> bool {
    workflow.is_some_and(|workflow| {
        workflow.archive_path != candidate
            && !matches!(workflow.source_fetch, CheatStepResource::NotLoaded)
    })
}


/// Drops every candidate-derived stage. Called whenever the archive,
/// profile, adapter, source mode, or catalogue snapshot changes, so a
/// candidate, cheat selection, or preview from one context can never be
/// shown - or applied - against another.
pub(crate) fn clear_cheat_candidate_state(workflow: &mut CheatWorkflowState) {
    workflow.candidates = CheatStepResource::NotLoaded;
    workflow.candidates_request = None;
    workflow.candidate_query.clear();
    workflow.candidate_selection = None;
    workflow.candidate_load_error = None;
    workflow.preview_request = None;
    workflow.preview = CheatStepResource::NotLoaded;
    workflow.transaction = CheatTransactionState::Idle;
}


pub(crate) fn cheat_picker_row_matches(
    row: &ArchiveRow,
    search: &str,
    platform_filter: Option<&str>,
    source_filter: Option<&Path>,
) -> bool {
    if row.origin != RowOrigin::Live {
        return false;
    }
    let normalized = search.trim().to_lowercase();
    let source_matches =
        source_filter.is_none_or(|wanted| row.source_path.as_deref() == Some(wanted));
    let platform_matches = platform_filter.is_none_or(|wanted| row.platform == wanted);
    let text_matches = normalized.is_empty()
        || row.matches(&normalized)
        || row.source_path.as_ref().is_some_and(|source| {
            source
                .to_string_lossy()
                .to_lowercase()
                .contains(&normalized)
        });
    source_matches && platform_matches && text_matches
}


pub(crate) fn cheat_picker_visible_indices(
    rows: &[ArchiveRow],
    picker: &CheatArchivePickerState,
) -> Vec<usize> {
    let mut indices: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            cheat_picker_row_matches(
                row,
                &picker.search,
                picker.platform_filter.as_deref(),
                picker.source_filter.as_deref(),
            )
        })
        .map(|(index, _)| index)
        .collect();
    indices.sort_by(|left, right| rows[*left].path.cmp(&rows[*right].path));
    indices
}


pub(crate) fn move_cheat_picker_candidate(
    rows: &[ArchiveRow],
    visible: &[usize],
    current: Option<&Path>,
    direction: ArrowDirection,
) -> Option<PathBuf> {
    if visible.is_empty() {
        return None;
    }
    let current_position = current.and_then(|current| {
        visible
            .iter()
            .position(|index| rows[*index].path == current)
    });
    let position = match (current_position, direction) {
        (Some(position), ArrowDirection::Down) => (position + 1).min(visible.len() - 1),
        (Some(position), ArrowDirection::Up) => position.saturating_sub(1),
        (None, ArrowDirection::Down) => 0,
        (None, ArrowDirection::Up) => visible.len() - 1,
    };
    Some(rows[visible[position]].path.clone())
}


pub(crate) fn show_cheat_archive_picker(
    context: &egui::Context,
    picker: &mut CheatArchivePickerState,
    rows: &[ArchiveRow],
    shared_platform: &mut Option<String>,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<CheatArchivePickerAction> {
    let default_size = cheat_archive_picker_size(context.input(|input| input.screen_rect().size()));
    let mut action = None;
    let mut open = true;
    egui::Window::new("Choose an archive for Cheats & Mods")
        .id(egui::Id::new("cheats_mods_archive_picker"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size(default_size)
        .min_size(egui::vec2(520.0, 420.0))
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(context, |ui| {
            ui.label("This changes only the Cheats & Mods context. It does not mount, queue, fetch, or modify the archive.");
            let search_has_focus = ui
                .add(
                egui::TextEdit::singleline(&mut picker.search)
                    .hint_text("Search name, platform, source, mount state, or path")
                    .desired_width(f32::INFINITY),
                )
                .has_focus();

            let mut platforms: Vec<String> = rows
                .iter()
                .filter(|row| row.origin == RowOrigin::Live)
                .map(|row| row.platform.clone())
                .collect();
            platforms.sort();
            platforms.dedup();
            let mut sources: Vec<PathBuf> = rows
                .iter()
                .filter(|row| row.origin == RowOrigin::Live)
                .filter_map(|row| row.source_path.clone())
                .collect();
            sources.sort();
            sources.dedup();
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("cheat_picker_platform")
                    .selected_text(picker.platform_filter.as_deref().unwrap_or("All platforms"))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut picker.platform_filter, None, "All platforms");
                        for platform in &platforms {
                            ui.selectable_value(
                                &mut picker.platform_filter,
                                Some(platform.clone()),
                                platform,
                            );
                        }
                    });
                egui::ComboBox::from_id_salt("cheat_picker_source")
                    .selected_text(
                        picker
                            .source_filter
                            .as_ref()
                            .map(|path| path.display().to_string())
                            .unwrap_or_else(|| "All sources".to_string()),
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut picker.source_filter, None, "All sources");
                        for source in &sources {
                            ui.selectable_value(
                                &mut picker.source_filter,
                                Some(source.clone()),
                                source.display().to_string(),
                            );
                        }
                    });
            });
            *shared_platform = picker.platform_filter.clone();

            let visible = cheat_picker_visible_indices(rows, picker);
            if !search_has_focus && ui.input(|input| input.key_pressed(egui::Key::ArrowDown)) {
                picker.candidate = move_cheat_picker_candidate(
                    rows,
                    &visible,
                    picker.candidate.as_deref(),
                    ArrowDirection::Down,
                );
            }
            if !search_has_focus && ui.input(|input| input.key_pressed(egui::Key::ArrowUp)) {
                picker.candidate = move_cheat_picker_candidate(
                    rows,
                    &visible,
                    picker.candidate.as_deref(),
                    ArrowDirection::Up,
                );
            }

            ui.separator();
            ui.label(format!("{} matching archives", visible.len()));
            egui::ScrollArea::vertical()
                .id_salt("cheat_archive_picker_rows")
                .max_height((ui.available_height() * 0.52).max(160.0))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for index in &visible {
                        let row = &rows[*index];
                        let title = row
                            .path
                            .file_name()
                            .map(|name| name.to_string_lossy())
                            .unwrap_or_else(|| row.path.as_os_str().to_string_lossy());
                        let selected = picker.candidate.as_deref() == Some(row.path.as_path());
                        let response = ui
                            .add(
                                egui::Button::selectable(
                                selected,
                                format!("{title}  ·  {}  ·  {}", row.platform, row.state),
                                )
                                .truncate(),
                            )
                            .on_hover_text(row.path.display().to_string());
                        if response.clicked() {
                            picker.candidate = Some(row.path.clone());
                        }
                        response.context_menu(|ui| {
                            if ui.button("Copy archive path").clicked() {
                                let _ = clipboard.set_text(row.path.display().to_string());
                                ui.close();
                            }
                        });
                    }
                });

            if let Some(candidate) = picker.candidate.as_ref()
                && let Some(row) = rows.iter().find(|row| row.path == *candidate)
            {
                widgets::card(ui, |ui| {
                    ui.strong("Selected archive preview");
                    ui.horizontal_wrapped(|ui| {
                        widgets::status_badge(ui, &row.platform, widgets::StatusTone::Info);
                        widgets::status_badge(ui, &row.state, widgets::StatusTone::Pending);
                    });
                    if widgets::path_value(ui, "Archive", &row.path) {
                        let _ = clipboard.set_text(row.path.display().to_string());
                    }
                    if let Some(source) = &row.source_path
                        && widgets::path_value(ui, "Source", source)
                    {
                        let _ = clipboard.set_text(source.display().to_string());
                    }
                    match row.path.extension().and_then(|value| value.to_str()) {
                        Some(extension) if extension.eq_ignore_ascii_case("rvz") => {
                            ui.label("Identity: RVZ is visible from platform evidence; exact Game ID extraction is not available yet.");
                        }
                        Some(extension)
                            if ["iso", "gcm", "gcz", "wbfs", "ciso"]
                                .iter()
                                .any(|candidate| extension.eq_ignore_ascii_case(candidate)) =>
                        {
                            ui.label("Identity: exact disc identity is checked after selection; the item stays visible if inspection is unavailable.");
                        }
                        _ if row.unknown_platform => {
                            ui.label("Unknown because no header identity, source assignment, folder alias, or filename evidence matched.");
                        }
                        _ => {}
                    }
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    action = Some(CheatArchivePickerAction::Cancel);
                }
                let choose = ui.add_enabled(
                    picker.candidate.as_ref().is_some_and(|candidate| {
                        rows.iter().any(|row| {
                            row.origin == RowOrigin::Live && row.path == *candidate
                        })
                    }),
                    egui::Button::new("Use selected archive"),
                );
                if choose.clicked()
                    || (!search_has_focus
                        && choose.enabled()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter)))
                {
                    action = picker
                        .candidate
                        .clone()
                        .map(CheatArchivePickerAction::Select);
                }
            });
        });
    if !open && action.is_none() {
        action = Some(CheatArchivePickerAction::Cancel);
    }
    if action.is_none() && context.input(|input| input.key_pressed(egui::Key::Escape)) {
        action = Some(CheatArchivePickerAction::Cancel);
    }
    action
}


pub(crate) fn cheat_archive_picker_size(screen: egui::Vec2) -> egui::Vec2 {
    egui::vec2(
        (screen.x * 0.82).clamp(620.0, 1080.0),
        (screen.y * 0.82).clamp(480.0, 760.0),
    )
}


/// Display label for a cached trusted-source snapshot's freshness.
pub(crate) fn cheat_freshness_label(freshness: CheatSourceFreshness) -> &'static str {
    match freshness {
        CheatSourceFreshness::Fresh => "Fresh",
        CheatSourceFreshness::Stale => "Stale",
        CheatSourceFreshness::Missing => "Not cached",
        CheatSourceFreshness::Unknown => "Unknown",
    }
}


pub(crate) fn cheat_freshness_tone(freshness: CheatSourceFreshness) -> widgets::StatusTone {
    match freshness {
        CheatSourceFreshness::Fresh => widgets::StatusTone::Success,
        CheatSourceFreshness::Stale => widgets::StatusTone::Warning,
        CheatSourceFreshness::Missing | CheatSourceFreshness::Unknown => {
            widgets::StatusTone::Pending
        }
    }
}


/// Display label for how a catalogue retrieval was satisfied.
pub(crate) fn cheat_fetch_status_label(status: CheatSourceFetchStatus) -> &'static str {
    match status {
        CheatSourceFetchStatus::Fetched => "Downloaded fresh catalogue",
        CheatSourceFetchStatus::CacheReused => "Reused cached snapshot",
        CheatSourceFetchStatus::OfflineReused => "Offline: reused cached snapshot",
    }
}


/// Builds the destination-bound code selection for a ready Dolphin
/// provider result - shared by the background-fetch completion handler and
/// `try_resolve_dolphin_provider_from_local_sources`, so both paths land
/// on exactly the same selection/error behavior regardless of whether the
/// result came from a network fetch or a purely local lookup.
pub(crate) fn build_dolphin_provider_selection(
    dolphin_profile_paths: &HashMap<String, PathBuf>,
    selected_dolphin_profile_id: Option<&str>,
    fetch: &GeckoProviderFetchResult,
) -> (Option<DolphinProviderSelectionState>, Option<String>) {
    let selection = selected_dolphin_profile_id
        .and_then(|profile_id| dolphin_profile_paths.get(profile_id))
        .map(|configuration_path| {
            load_dolphin_destination(configuration_path, &fetch.result.game_id)
        });
    match selection {
        Some(Ok(destination)) => {
            let codes = DolphinProviderCodeSelection::from_provider(&fetch.result, &destination);
            (
                Some(DolphinProviderSelectionState {
                    destination,
                    selection: codes,
                }),
                None,
            )
        }
        Some(Err(error)) => (None, Some(error.to_string())),
        None => (
            None,
            Some("Choose an eligible Dolphin profile before selecting provider codes.".to_string()),
        ),
    }
}


/// A short, non-technical name for a recognised disc image format, used
/// only in the beginner-facing "exact Game ID unavailable" message. Full
/// transport/parse diagnostics remain under Details, never here.
pub(crate) fn dolphin_identity_format_label(format: IdentityImageFormat) -> &'static str {
    match format {
        IdentityImageFormat::Iso => "This GameCube/Wii image",
        IdentityImageFormat::ZipContainingIso => "This ZIP archive",
        IdentityImageFormat::Rvz => "This RVZ file",
        IdentityImageFormat::Ciso => "This CISO file",
        IdentityImageFormat::Wbfs => "This WBFS file",
        IdentityImageFormat::Chd => "This disc image format",
        IdentityImageFormat::Deferred => "This disc image format",
        IdentityImageFormat::Gdi
        | IdentityImageFormat::Cdi
        | IdentityImageFormat::LooseCartridgeRom
        | IdentityImageFormat::Xex
        | IdentityImageFormat::ZipContainingXex
        | IdentityImageFormat::Xbe
        | IdentityImageFormat::ZipContainingXbe
        | IdentityImageFormat::XboxDiscImage
        | IdentityImageFormat::Pkg
        | IdentityImageFormat::ScummVmDirectory
        | IdentityImageFormat::Pbp
        | IdentityImageFormat::Unsupported => "This file",
    }
}


/// The beginner-facing detail line for `BeginnerCheatStatus::IdentityUnavailable` -
/// derived entirely from the same `GameIdentityReport` Details already
/// shows, so the two views can never disagree. Never fabricates identity
/// from the filename and never suggests mounting will definitely help.
pub(crate) fn dolphin_identity_unavailable_detail(report: &GameIdentityReport) -> String {
    let format_label = dolphin_identity_format_label(report.format);
    let status = report
        .evidence
        .iter()
        .find(|item| item.kind == IdentityKind::DolphinGameId)
        .map(|item| item.status);
    match status {
        Some(IdentityStatus::Invalid) => format!(
            "{format_label} is recognised as a GameCube/Wii image, but its disc header could not be verified. The file may be malformed or use an unrecognised layout."
        ),
        Some(IdentityStatus::Deferred) => format!(
            "{format_label} is recognised as a GameCube/Wii image, but EmuWiz cannot yet read an exact Game ID from it without decompressing the full image. Cheats cannot be matched safely without one."
        ),
        Some(IdentityStatus::Missing) => {
            format!("{format_label} is recognised, but the disc-header block is not present in it.")
        }
        Some(IdentityStatus::Unsupported) | None => format!(
            "{format_label} is recognised as a GameCube/Wii game, but exact identity extraction is not supported for it yet."
        ),
        Some(_) => "Exact Game ID is not yet available for this file.".to_string(),
    }
}


pub(crate) fn dolphin_identity_row_state(workflow: &CheatWorkflowState) -> DolphinIdentityRowState<'_> {
    match ready_game_identity(workflow) {
        Some(report) => match report.verified_dolphin_game_id() {
            Some(id) => DolphinIdentityRowState::Verified(id),
            None => DolphinIdentityRowState::Unavailable,
        },
        None => DolphinIdentityRowState::Pending,
    }
}


pub(crate) fn dolphin_provider_fetch_status_label(status: GeckoProviderFetchStatus) -> &'static str {
    match status {
        GeckoProviderFetchStatus::Downloaded => "downloaded",
        GeckoProviderFetchStatus::FreshCache => "fresh cache",
        GeckoProviderFetchStatus::RateLimitedCache => "rate-limited cache",
        GeckoProviderFetchStatus::StaleCacheFallback => "stale cache fallback",
        GeckoProviderFetchStatus::Catalogue => "local Dolphin cheat catalogue",
        GeckoProviderFetchStatus::NotAvailable => "no upstream file for this game",
    }
}


/// Computes the beginner status for Dolphin from the same state the
/// technical Details view already reads - no separate tracking, so the
/// two views can never disagree about what actually happened.
pub(crate) fn dolphin_beginner_status(workflow: &CheatWorkflowState) -> BeginnerCheatStatus {
    match &workflow.dolphin_profile_selection {
        None | Some(EmulatorProfileSelection::SetupNeeded) => {
            return BeginnerCheatStatus::EmulatorSetupNeeded;
        }
        Some(EmulatorProfileSelection::NeedsChoice { .. }) => {
            return BeginnerCheatStatus::ChooseEmulatorProfile;
        }
        Some(EmulatorProfileSelection::Auto { .. }) => {}
    }
    if workflow.platform.as_deref() == Some("Wii") {
        return wii_gamehacking_beginner_status(workflow);
    }
    // Identity has reached a final result but never produced a `Verified`
    // exact game ID (malformed image, or a recognised format EmuWiz
    // cannot yet decode - see `dolphin_identity_unavailable_detail`).
    // `start_dolphin_provider_fetch`/`try_resolve_dolphin_provider_from_local_sources`
    // both require a verified game ID before doing anything, so without
    // this check `dolphin_provider` would stay `NotLoaded` forever and the
    // page would show "Finding compatible cheats" indefinitely instead of
    // this honest terminal state.
    if let Some(report) = ready_game_identity(workflow)
        && report.verified_dolphin_game_id().is_none()
    {
        return BeginnerCheatStatus::IdentityUnavailable {
            detail: dolphin_identity_unavailable_detail(report),
        };
    }
    match &workflow.dolphin_provider {
        CheatStepResource::NotLoaded => match workflow.dolphin_local_lookup {
            DolphinLocalLookupState::NotAttempted => BeginnerCheatStatus::FindingCompatibleCheats,
            // The local catalogue/cache lookup already ran and found
            // nothing; the Dolphin catalogue card explains why and offers
            // the fix, so this stays the same honest "nothing found"
            // wording rather than a spinner that would never resolve.
            _ => BeginnerCheatStatus::NoCompatibleCheatsFound,
        },
        CheatStepResource::Loading { .. } => BeginnerCheatStatus::FindingCompatibleCheats,
        CheatStepResource::Failed(message) => BeginnerCheatStatus::CouldNotCheckForCheats {
            detail: message.clone(),
        },
        CheatStepResource::Ready(fetch) => {
            if fetch.status == GeckoProviderFetchStatus::NotAvailable {
                return BeginnerCheatStatus::NoUpstreamCheatsAvailable;
            }
            if fetch.status == GeckoProviderFetchStatus::StaleCacheFallback {
                return BeginnerCheatStatus::UsingSavedResultsWhileOffline;
            }
            let compatible_count = workflow
                .dolphin_provider_selection
                .as_ref()
                .map(|state| state.selection.selectable_count())
                .unwrap_or(0);
            if compatible_count == 0 {
                BeginnerCheatStatus::NoCompatibleCheatsFound
            } else {
                BeginnerCheatStatus::CheatsFound { compatible_count }
            }
        }
    }
}


pub(crate) fn wii_gamehacking_beginner_status(workflow: &CheatWorkflowState) -> BeginnerCheatStatus {
    match &workflow.gamecube_gamehacking {
        CheatStepResource::NotLoaded | CheatStepResource::Loading { .. } => {
            BeginnerCheatStatus::FindingCompatibleCheats
        }
        CheatStepResource::Failed(message) => BeginnerCheatStatus::CouldNotCheckForCheats {
            detail: message.clone(),
        },
        CheatStepResource::Ready(state) if state.cached_fallback => {
            BeginnerCheatStatus::UsingSavedResultsWhileOffline
        }
        CheatStepResource::Ready(state) => {
            let compatible_count = state
                .selection
                .entries
                .iter()
                .filter(|entry| entry.selectable)
                .count();
            if compatible_count == 0 {
                BeginnerCheatStatus::NoCompatibleCheatsFound
            } else {
                BeginnerCheatStatus::CheatsFound { compatible_count }
            }
        }
    }
}


/// Xenia's counterpart to `dolphin_beginner_status`.
pub(crate) fn xenia_beginner_status(workflow: &CheatWorkflowState) -> BeginnerCheatStatus {
    match &workflow.xenia_profile_selection {
        None | Some(EmulatorProfileSelection::SetupNeeded) => {
            return BeginnerCheatStatus::EmulatorSetupNeeded;
        }
        Some(EmulatorProfileSelection::NeedsChoice { .. }) => {
            return BeginnerCheatStatus::ChooseEmulatorProfile;
        }
        Some(EmulatorProfileSelection::Auto { .. }) => {}
    }
    match &workflow.xenia_provider {
        CheatStepResource::NotLoaded | CheatStepResource::Loading { .. } => {
            BeginnerCheatStatus::FindingCompatibleCheats
        }
        CheatStepResource::Failed(message) => BeginnerCheatStatus::CouldNotCheckForCheats {
            detail: message.clone(),
        },
        CheatStepResource::Ready(fetch) => {
            if fetch.status == XeniaProviderFetchStatus::StaleCacheFallback {
                return BeginnerCheatStatus::UsingSavedResultsWhileOffline;
            }
            let compatible_count = workflow
                .xenia_selection
                .as_ref()
                .filter(|state| {
                    state.selection.compatibility != XeniaCandidateCompatibility::Incompatible
                })
                .map(|state| state.selection.selectable_count())
                .unwrap_or(0);
            if compatible_count == 0 {
                BeginnerCheatStatus::NoCompatibleCheatsFound
            } else {
                BeginnerCheatStatus::CheatsFound { compatible_count }
            }
        }
    }
}


pub(crate) fn summarise_cheat_warnings(warnings: &[String]) -> Vec<String> {
    warnings
        .iter()
        .map(|warning| {
            let count = warning.split_whitespace().next().unwrap_or("Some");
            if warning.contains("retained but are non-actionable because parsing was incomplete") {
                format!(
                    "{count} catalogue files could not be parsed and were excluded from matching."
                )
            } else if warning.contains("unsupported content or encoding") {
                format!("{count} files used unsupported content encoding and were excluded.")
            } else if warning.contains("paths used unsupported encoding") {
                format!("{count} paths used unsupported encoding and were excluded safely.")
            } else if warning == "cached snapshot is stale" {
                "The cached catalogue is stale; update it when a network connection is available."
                    .to_string()
            } else {
                warning.clone()
            }
        })
        .collect()
}


/// Renders catalogue-indexing warnings as a concise, bounded summary
/// instead of dumping every entry directly into the page - the Sources
/// page's original complaint was thousands of malformed/unsupported
/// cheat-file diagnostics rendered as one banner each, unbounded, right
/// in the normal workflow. Shows a compact "N catalogue issues found, the
/// catalogue still works" banner, with a bounded explanation
/// representative entries, with the complete list (plus a "Copy all"
/// action) always reachable behind `technical_details` - no diagnostic
/// data is discarded, only its default on-screen footprint is bounded.
pub(crate) fn show_cheat_warnings_summary(
    ui: &mut egui::Ui,
    warnings: &[String],
    id_salt: impl std::hash::Hash,
    clipboard: &mut dyn ClipboardBackend,
) {
    const SAMPLE_LIMIT: usize = 3;
    if warnings.is_empty() {
        return;
    }
    let summarised = summarise_cheat_warnings(warnings);
    widgets::banner(
        ui,
        &format!(
            "{} catalogue issue{} found",
            summarised.len(),
            if summarised.len() == 1 { "" } else { "s" }
        ),
        "The catalogue still works. These files were skipped.",
        widgets::StatusTone::Warning,
    );
    // The exact "First use of widget ID .../Second use of widget ID ..."
    // collision this fixes: without an `id_salt`, every "What happened?"
    // header in this file shares one identical, literal-text-derived ID -
    // any two calls to this function rendered in the same frame (e.g. two
    // catalogue sources' warning sections both visible at once) collided.
    // `id_salt` is already unique per call site (source ID, archive SHA,
    // resolved commit, ...); it was already passed to the *inner*
    // `technical_details` disclosure below, just never to this outer one.
    egui::CollapsingHeader::new("What happened?")
        .id_salt(&id_salt)
        .default_open(false)
        .show(ui, |ui| {
            for warning in summarised.iter().take(SAMPLE_LIMIT) {
                ui.label(format!("• {warning}"));
            }
            if summarised.len() > SAMPLE_LIMIT {
                ui.weak(format!(
                    "+ {} more - see Technical details below.",
                    summarised.len() - SAMPLE_LIMIT
                ));
            }
            widgets::technical_details(ui, id_salt, |ui| {
                if widgets::action_button(ui, "Copy all", widgets::ActionStyle::Quiet, true)
                    .clicked()
                {
                    let _ = clipboard.set_text(warnings.join("\n"));
                }
                for warning in warnings {
                    ui.label(format!("• {warning}"));
                }
            });
        });
}


pub(crate) fn import_trust_label(state: ImportTrustState) -> &'static str {
    match state {
        ImportTrustState::Trusted => "Trusted",
        ImportTrustState::Unverified => "Unverified",
        ImportTrustState::Blocked => "Blocked",
    }
}


pub(crate) fn import_trust_tone(state: ImportTrustState) -> widgets::StatusTone {
    match state {
        ImportTrustState::Trusted => widgets::StatusTone::Success,
        ImportTrustState::Unverified => widgets::StatusTone::Warning,
        ImportTrustState::Blocked => widgets::StatusTone::Blocked,
    }
}


pub(crate) fn import_source_presentation(kind: ImportSourceKind) -> (&'static str, &'static str) {
    match kind {
        ImportSourceKind::EmulatorManagedLibrary => {
            ("Existing emulator-managed library", "Existing content")
        }
        ImportSourceKind::ArchiveFsTrustedCatalogue => ("EmuWiz trusted catalogue", "Available"),
        ImportSourceKind::LocalUnverifiedSource => ("Local unverified source", "Planned"),
        ImportSourceKind::RemoteUnverifiedSource => ("Future remote unverified source", "Planned"),
    }
}


pub(crate) fn local_scanning_presentation(
    state: LocalSafetyScanningState,
) -> (&'static str, widgets::StatusTone) {
    match state {
        LocalSafetyScanningState::PlannedUnavailable => (
            "Local safety scanning · Planned",
            widgets::StatusTone::Pending,
        ),
        LocalSafetyScanningState::Enabled => {
            ("Local safety scanning · On", widgets::StatusTone::Success)
        }
        LocalSafetyScanningState::DisabledPendingConfirmation => (
            "Local safety scanning · Confirmation required",
            widgets::StatusTone::Warning,
        ),
        LocalSafetyScanningState::Disabled => {
            ("Local safety scanning · Off", widgets::StatusTone::Warning)
        }
    }
}


pub(crate) fn show_cheats_mods_workflow_states(
    ui: &mut egui::Ui,
    workflow: Option<&CheatWorkflowState>,
    profiles: &RetroArchProfilesState,
    pcsx2_profiles: &Pcsx2ProfilesState,
    dolphin_profiles: &DolphinProfilesState,
) {
    if workflow.is_some_and(|workflow| workflow.adapter == CheatEmulatorAdapter::Pcsx2) {
        show_pcsx2_workflow_states(ui, workflow.unwrap(), pcsx2_profiles);
        return;
    }
    if workflow.is_some_and(|workflow| workflow.adapter == CheatEmulatorAdapter::Dolphin) {
        show_dolphin_workflow_states(ui, workflow.unwrap(), dolphin_profiles);
        return;
    }
    if workflow.is_some_and(|workflow| workflow.adapter == CheatEmulatorAdapter::Xenia) {
        let workflow = workflow.unwrap();
        widgets::status_strip(
            ui,
            &[(
                "Xenia Canary profile",
                if workflow.selected_xenia_profile_id.is_some() {
                    widgets::StatusTone::Success
                } else {
                    widgets::StatusTone::Pending
                },
            )],
        );
        return;
    }
    if workflow.is_some_and(|workflow| workflow.adapter == CheatEmulatorAdapter::Unsupported) {
        widgets::banner(
            ui,
            "Unsupported platform",
            "No emulator adapter, source, preview, or transaction is active for this archive.",
            widgets::StatusTone::Warning,
        );
        return;
    }
    let (profile_label, profile_tone) = retroarch_integration_presentation(profiles);
    let source_label = workflow
        .map(|workflow| workflow.source_mode.label())
        .unwrap_or("No source mode selected");
    let (source_tone, trust_label, trust_tone) = match workflow.map(|workflow| workflow.source_mode)
    {
        Some(CheatSourceMode::ExistingRetroArchLibrary) => (
            widgets::StatusTone::Info,
            import_trust_label(ImportTrustState::Unverified),
            import_trust_tone(ImportTrustState::Unverified),
        ),
        Some(CheatSourceMode::ArchiveFsTrustedCatalogue) => (
            widgets::StatusTone::Success,
            import_trust_label(ImportTrustState::Trusted),
            import_trust_tone(ImportTrustState::Trusted),
        ),
        None => (
            widgets::StatusTone::Pending,
            "Not selected",
            widgets::StatusTone::Pending,
        ),
    };
    let (inspection_label, inspection_tone) = match workflow {
        Some(workflow) if workflow.source_mode == CheatSourceMode::ExistingRetroArchLibrary => {
            match &workflow.existing_library {
                CheatStepResource::Ready(result) if result.complete => (
                    "Local bounded inventory complete",
                    widgets::StatusTone::Success,
                ),
                CheatStepResource::Ready(_) => {
                    ("Local inventory incomplete", widgets::StatusTone::Warning)
                }
                CheatStepResource::Loading { .. } => {
                    ("Inspecting locally", widgets::StatusTone::Active)
                }
                CheatStepResource::Failed(_) => {
                    ("Local inventory unavailable", widgets::StatusTone::Warning)
                }
                CheatStepResource::NotLoaded => {
                    ("Local inventory pending", widgets::StatusTone::Pending)
                }
            }
        }
        Some(_) => (
            "Trusted retrieval validation available",
            widgets::StatusTone::Success,
        ),
        None => ("Not started", widgets::StatusTone::Pending),
    };
    let destination_label = workflow
        .and_then(|workflow| {
            let selected = workflow.selected_profile_id.as_deref()?;
            let RetroArchProfilesState::Ready(discovery) = profiles else {
                return None;
            };
            discovery
                .profiles
                .iter()
                .find(|profile| profile.eligible && profile.profile_id == selected)
                .and_then(|profile| profile.cheat_destination_root.as_ref())
                .map(|path| path.display.clone())
        })
        .unwrap_or_else(|| {
            if workflow.is_some() {
                "Not selected — choose an eligible profile".to_string()
            } else {
                "No archive context".to_string()
            }
        });
    widgets::section_header(
        ui,
        "Workflow state",
        Some(
            "Profile, source, inspection, destination, and installation remain separate decisions.",
        ),
    );
    widgets::status_rows(
        ui,
        &[
            ("Emulator profile", profile_label.as_str(), profile_tone),
            ("Cheat or mod source", source_label, source_tone),
            ("Trust state", trust_label, trust_tone),
            ("Inspection state", inspection_label, inspection_tone),
            (
                "Destination",
                destination_label.as_str(),
                widgets::StatusTone::Pending,
            ),
            (
                "Installation state",
                if workflow.is_some_and(|workflow| {
                    workflow.source_mode == CheatSourceMode::ArchiveFsTrustedCatalogue
                }) {
                    "Controlled apply available after eligible preview"
                } else {
                    "Unavailable for this source mode"
                },
                if workflow.is_some_and(|workflow| {
                    workflow.source_mode == CheatSourceMode::ArchiveFsTrustedCatalogue
                }) {
                    widgets::StatusTone::Info
                } else {
                    widgets::StatusTone::Pending
                },
            ),
        ],
    );
}


pub(crate) fn show_pcsx2_workflow_states(
    ui: &mut egui::Ui,
    workflow: &CheatWorkflowState,
    profiles: &Pcsx2ProfilesState,
) {
    let (profile_label, profile_tone) = pcsx2_integration_presentation(profiles);
    let (inspection_label, inspection_tone) = match &workflow.pcsx2_inventory {
        CheatStepResource::Ready(inventory) if inventory.complete => (
            "Local PNACH inventory complete",
            widgets::StatusTone::Success,
        ),
        CheatStepResource::Ready(_) => (
            "Local PNACH inventory incomplete",
            widgets::StatusTone::Warning,
        ),
        CheatStepResource::Loading { .. } => ("Inspecting locally", widgets::StatusTone::Active),
        CheatStepResource::Failed(_) => {
            ("Local inspection unavailable", widgets::StatusTone::Warning)
        }
        CheatStepResource::NotLoaded => ("Local inspection pending", widgets::StatusTone::Pending),
    };
    let destination = workflow
        .selected_pcsx2_profile_id
        .as_deref()
        .and_then(|selected| match profiles {
            Pcsx2ProfilesState::Ready(discovery) => discovery
                .profiles
                .iter()
                .find(|profile| profile.eligible && profile.profile_id == selected)
                .map(|profile| profile.configuration_path.display().to_string()),
            _ => None,
        })
        .unwrap_or_else(|| "Not selected — choose an eligible profile".to_string());
    widgets::section_header(
        ui,
        "Workflow state",
        Some("Profile, source, inspection, destination, and installation remain separate states."),
    );
    widgets::status_rows(
        ui,
        &[
            ("Emulator profile", profile_label.as_str(), profile_tone),
            (
                "Cheat or mod source",
                "Existing PCSX2-managed files",
                widgets::StatusTone::Info,
            ),
            (
                "Trust state",
                "Unverified local content",
                widgets::StatusTone::Warning,
            ),
            ("Inspection state", inspection_label, inspection_tone),
            (
                "Destination",
                destination.as_str(),
                widgets::StatusTone::Pending,
            ),
            (
                "Installation state",
                "Unavailable · read-only adapter",
                widgets::StatusTone::Pending,
            ),
        ],
    );
}


pub(crate) fn pcsx2_integration_presentation(profiles: &Pcsx2ProfilesState) -> (String, widgets::StatusTone) {
    match profiles {
        Pcsx2ProfilesState::NotScanned => (
            "PCSX2 profiles not scanned".to_string(),
            widgets::StatusTone::Pending,
        ),
        Pcsx2ProfilesState::Scanning { .. } => (
            "Scanning PCSX2 profiles".to_string(),
            widgets::StatusTone::Active,
        ),
        Pcsx2ProfilesState::Error(_) => (
            "PCSX2 profile scan needs attention".to_string(),
            widgets::StatusTone::Blocked,
        ),
        Pcsx2ProfilesState::Ready(discovery) => {
            let eligible = discovery
                .profiles
                .iter()
                .filter(|profile| profile.eligible)
                .count();
            if eligible == 0 {
                (
                    "No eligible PCSX2 profile".to_string(),
                    widgets::StatusTone::Warning,
                )
            } else {
                (
                    format!(
                        "{eligible} eligible PCSX2 profile{}",
                        if eligible == 1 { "" } else { "s" }
                    ),
                    widgets::StatusTone::Success,
                )
            }
        }
    }
}


pub(crate) fn show_dolphin_workflow_states(
    ui: &mut egui::Ui,
    workflow: &CheatWorkflowState,
    profiles: &DolphinProfilesState,
) {
    let (profile_label, profile_tone) = dolphin_integration_presentation(profiles);
    let (inspection_label, inspection_tone) = match &workflow.dolphin_inventory {
        CheatStepResource::Ready(inventory) if inventory.complete => (
            "Local Game INI inventory complete",
            widgets::StatusTone::Success,
        ),
        CheatStepResource::Ready(_) => (
            "Local Game INI inventory incomplete",
            widgets::StatusTone::Warning,
        ),
        CheatStepResource::Loading { .. } => ("Inspecting locally", widgets::StatusTone::Active),
        CheatStepResource::Failed(_) => {
            ("Local inspection unavailable", widgets::StatusTone::Warning)
        }
        CheatStepResource::NotLoaded => ("Local inspection pending", widgets::StatusTone::Pending),
    };
    let destination = workflow
        .selected_dolphin_profile_id
        .as_deref()
        .and_then(|selected| match profiles {
            DolphinProfilesState::Ready(discovery) => discovery
                .profiles
                .iter()
                .find(|profile| profile.eligible && profile.profile_id == selected)
                .map(|profile| profile.game_settings_path.display().to_string()),
            _ => None,
        })
        .unwrap_or_else(|| "Not selected — choose an eligible profile".to_string());
    widgets::section_header(
        ui,
        "Workflow state",
        Some(
            "Profile, inspection, identity, destination, and installation remain separate states.",
        ),
    );
    widgets::status_rows(
        ui,
        &[
            ("Emulator profile", profile_label.as_str(), profile_tone),
            (
                "Cheat or mod source",
                "Dolphin upstream GameSettings provider",
                widgets::StatusTone::Info,
            ),
            (
                "Trust state",
                "Exact-ID provider data · locally validated",
                widgets::StatusTone::Info,
            ),
            ("Inspection state", inspection_label, inspection_tone),
            (
                "Destination",
                destination.as_str(),
                widgets::StatusTone::Pending,
            ),
            (
                "Installation state",
                "Preview, journal-backed apply, and rollback available",
                widgets::StatusTone::Success,
            ),
        ],
    );
}


pub(crate) fn dolphin_integration_presentation(
    profiles: &DolphinProfilesState,
) -> (String, widgets::StatusTone) {
    match profiles {
        DolphinProfilesState::NotScanned => (
            "Dolphin profiles not scanned".to_string(),
            widgets::StatusTone::Pending,
        ),
        DolphinProfilesState::Scanning { .. } => (
            "Scanning Dolphin profiles".to_string(),
            widgets::StatusTone::Active,
        ),
        DolphinProfilesState::Error(_) => (
            "Dolphin profile scan needs attention".to_string(),
            widgets::StatusTone::Blocked,
        ),
        DolphinProfilesState::Ready(discovery) => {
            let eligible = discovery
                .profiles
                .iter()
                .filter(|profile| profile.eligible)
                .count();
            if eligible == 0 {
                (
                    "No eligible Dolphin profile".to_string(),
                    widgets::StatusTone::Warning,
                )
            } else {
                (
                    format!(
                        "{eligible} eligible Dolphin profile{}",
                        if eligible == 1 { "" } else { "s" }
                    ),
                    widgets::StatusTone::Success,
                )
            }
        }
    }
}


pub(crate) fn show_cheats_mods_safety_information(ui: &mut egui::Ui) {
    egui::CollapsingHeader::new("Safety, privacy, and responsible use")
        .default_open(false)
        .show(ui, |ui| {
            widgets::card(ui, |ui| {
                ui.label(UNKNOWN_CODE_POLICY);
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    let (label, tone) =
                        local_scanning_presentation(LocalSafetyScanningState::current());
                    widgets::status_badge(ui, label, tone);
                    ui.label("No general local or community-source scanner or setting is active yet.");
                });
                ui.label(LOCAL_INSPECTION_PRIVACY_COPY);
                ui.label("EmuWiz never silently rewrites, deletes, or sanitizes an original import source. A future sanitized import would be a separate copy with an exclusion report.");
                ui.label(IMPORT_CONSENT_COPY);
                ui.label(format!("Future scanning control: {SCANNING_DISABLED_WARNING}"));
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    for state in [
                        ImportTrustState::Trusted,
                        ImportTrustState::Unverified,
                        ImportTrustState::Blocked,
                    ] {
                        widgets::status_badge(
                            ui,
                            import_trust_label(state),
                            import_trust_tone(state),
                        );
                    }
                });
                ui.label("Trusted means a reviewed adapter and known provenance. Unverified means not reviewed by EmuWiz; it does not mean malicious. Blocked is reserved for a concrete technical danger or adapter incompatibility.");
                ui.add_space(6.0);
                ui.label(ETHICAL_USE_COPY);
                ui.label(USER_RESPONSIBILITY_COPY);
            });
        });
}


pub(crate) fn retroarch_integration_presentation(
    profiles: &RetroArchProfilesState,
) -> (String, widgets::StatusTone) {
    match profiles {
        RetroArchProfilesState::NotScanned => (
            "RetroArch profiles not scanned".to_string(),
            widgets::StatusTone::Pending,
        ),
        RetroArchProfilesState::Scanning { .. } => (
            "Scanning RetroArch profiles".to_string(),
            widgets::StatusTone::Active,
        ),
        RetroArchProfilesState::Error(_) => (
            "RetroArch profile scan needs attention".to_string(),
            widgets::StatusTone::Blocked,
        ),
        RetroArchProfilesState::Ready(discovery) => {
            let eligible = discovery
                .profiles
                .iter()
                .filter(|profile| profile.eligible)
                .count();
            if eligible == 0 {
                (
                    "No eligible RetroArch profile".to_string(),
                    widgets::StatusTone::Warning,
                )
            } else {
                (
                    format!(
                        "{eligible} eligible RetroArch profile{}",
                        if eligible == 1 { "" } else { "s" }
                    ),
                    widgets::StatusTone::Success,
                )
            }
        }
    }
}


/// The "Technical details" body for the RetroArch core-folder card. This is
/// the only place raw `PathFinding` provenance, the `ResolutionState` name,
/// diagnostic codes and full filesystem paths appear - the normal summary
/// above deliberately shows none of them.
pub(crate) fn show_retroarch_core_folder_technical(
    ui: &mut egui::Ui,
    profiles: &RetroArchProfilesState,
    mode: &retroarch_core_setup::CoreFolderMode,
    rejected_pick: Option<&Path>,
) {
    use archivefs_core::emulator_environment::retroarch::PathPurpose;

    ui.label(format!("Active source: {}", mode.label()));
    match mode.custom_path() {
        Some(path) => {
            ui.label(format!("Override path: {}", path.display()));
            ui.label("Resolution: EmuWizCoreDirectoryOverride");
        }
        None => {
            ui.label("Override path: (none - automatic detection)");
        }
    }
    if let Some(path) = rejected_pick {
        ui.label(format!("Last rejected pick: {}", path.display()));
    }

    match profiles {
        RetroArchProfilesState::NotScanned => {
            ui.label("No RetroArch core scan has run yet.");
        }
        RetroArchProfilesState::Scanning { .. } => {
            ui.label("RetroArch core scan in progress.");
        }
        RetroArchProfilesState::Error(message) => {
            ui.label(format!("Scan error: {message}"));
        }
        RetroArchProfilesState::Ready(discovery) => {
            let inventory = retroarch_core_setup::core_inventory(discovery);
            ui.label(format!(
                "Usable libretro cores: {} (of {} found) - platforms mapped: {}",
                inventory.usable_cores, inventory.total_cores, inventory.mapped_platforms
            ));
            if let Some(finding) = discovery
                .environment
                .profiles
                .iter()
                .flat_map(|profile| profile.paths.iter())
                .find(|finding| finding.purpose == PathPurpose::Cores)
            {
                if let Some(resolved) = &finding.resolved_path {
                    ui.label(format!("Resolved core directory: {}", resolved.display));
                }
                if let Some(configured) = &finding.configured_value {
                    ui.label(format!("retroarch.cfg configured value: {configured}"));
                }
            }
            let mut any_diagnostic = false;
            for diagnostic in discovery
                .environment
                .profiles
                .iter()
                .flat_map(|profile| profile.diagnostics.iter())
            {
                any_diagnostic = true;
                match retroarch_core_setup::humanize_retroarch_diagnostic(diagnostic.code) {
                    Some(plain) => ui.label(format!("{} - {plain}", diagnostic.code)),
                    None => ui.label(diagnostic.code.to_string()),
                };
            }
            for diagnostic in &discovery.diagnostics {
                any_diagnostic = true;
                match retroarch_core_setup::humanize_retroarch_diagnostic(diagnostic.code) {
                    Some(plain) => ui.label(format!("{} - {plain}", diagnostic.code)),
                    None => ui.label(diagnostic.code.to_string()),
                };
            }
            if !any_diagnostic {
                ui.label("No RetroArch diagnostics recorded.");
            }
        }
    }
}


pub(crate) fn show_cheat_archive_context(
    ui: &mut egui::Ui,
    workflow: &CheatWorkflowState,
    live: Option<&LoadedData>,
    cached: Option<&CachedLibrarySnapshot>,
    clipboard: &mut dyn ClipboardBackend,
) {
    let record = live.and_then(|data| {
        data.records
            .iter()
            .find(|record| record.mount_plan.archive.path == workflow.archive_path)
    });
    let persisted = selected_persisted_archive(cached, Some(&workflow.archive_path));
    widgets::section_header(
        ui,
        if record.is_some_and(|record| !record.is_mount_input()) {
            "Selected loose-ROM context"
        } else {
            "Selected archive context"
        },
        Some(
            "This exact library item remains selected; opening this workspace changes no mount, queue, or platform state.",
        ),
    );
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(&workflow.display_name)
                    .size(19.0)
                    .strong(),
            );
            widgets::status_badge(
                ui,
                persisted
                    .and_then(|archive| archive.platform.as_deref())
                    .or(workflow.platform.as_deref())
                    .unwrap_or("Unknown platform"),
                widgets::StatusTone::Info,
            );
            if let Some(record) = record {
                if !record.is_mount_input() {
                    widgets::status_badge(ui, "Media kind · Loose ROM", widgets::StatusTone::Info);
                }
                widgets::status_badge(
                    ui,
                    mount_validation_label(record.mount_state),
                    match record.mount_state {
                        MountState::Mounted => widgets::StatusTone::Active,
                        MountState::Pending => widgets::StatusTone::Success,
                        MountState::MountPathExists => widgets::StatusTone::Warning,
                        MountState::NotMountable => widgets::StatusTone::Info,
                    },
                );
            }
            if persisted.is_some_and(|archive| {
                archive.platform_source.as_deref() == Some(MANUAL_PLATFORM_SOURCE)
            }) {
                widgets::status_badge(ui, "Manual platform assignment", widgets::StatusTone::Info);
            }
            ui.label(egui::RichText::new(format_size(workflow.size_bytes)).color(theme::muted(ui)));
        });
        if widgets::path_value(
            ui,
            if record.is_some_and(|record| !record.is_mount_input()) {
                "ROM file"
            } else {
                "Archive"
            },
            &workflow.archive_path,
        ) {
            let _ = clipboard.set_text(workflow.archive_path.display().to_string());
        }
        if widgets::path_value(ui, "Source", &workflow.source_root) {
            let _ = clipboard.set_text(workflow.source_root.display().to_string());
        }
        if let Some(record) = record.filter(|record| record.is_mount_input())
            && widgets::path_value(ui, "Mount destination", &record.mount_plan.mount_path)
        {
            let _ = clipboard.set_text(record.mount_plan.mount_path.display().to_string());
        }
    });
}


pub(crate) fn show_recent_cheat_activity(
    ui: &mut egui::Ui,
    history: &OperationHistory,
    archive_path: Option<&Path>,
) {
    let entries: Vec<&HistoryEntry> = history
        .entries()
        .filter(|entry| {
            entry.action == ActivityAction::RetroArchProfileScan
                || entry.action == ActivityAction::Pcsx2ProfileScan
                || (matches!(
                    entry.action,
                    ActivityAction::CheatSourceRetrieval | ActivityAction::Pcsx2PnachInspection
                ) && archive_path.is_some()
                    && entry.archive_path.as_deref() == archive_path)
        })
        .take(4)
        .collect();
    egui::CollapsingHeader::new("Recent related activity")
        .id_salt("cheats-recent-related-activity")
        .default_open(false)
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(
                    "This session's emulator checks, local PNACH inspection, and catalogue retrieval.",
                )
                .color(theme::muted(ui))
                .small(),
            );
            if entries.is_empty() {
                ui.weak("No related activity has been recorded in this session.");
            } else {
                for entry in entries {
                    widgets::card(ui, |ui| {
                        widgets::activity_row_header(
                            ui,
                            entry.outcome.to_string(),
                            activity_outcome_tone(entry.outcome),
                            entry.action.to_string(),
                            Some(&format_history_timestamp(entry.timestamp)),
                            |_ui| {},
                        );
                        ui.add(egui::Label::new(&entry.message).truncate())
                            .on_hover_text(&entry.message);
                    });
                    ui.add_space(4.0);
                }
            }
        });
}


/// A compact, honest notice - not a full section with its own heading and
/// card - since Mods has no workflow of its own yet and must not occupy
/// prime space above the real, working RetroArch cheat workflow. See
/// `docs/CHEATS_MODS_FUNCTIONAL_REPAIR.md` for the deferred mod-adapter
/// design this notice points at.
pub(crate) fn show_mods_section(ui: &mut egui::Ui, pcsx2_read_only: bool, dolphin_read_only: bool) {
    let (tone, detail): (widgets::StatusTone, &str) = if pcsx2_read_only {
        (
            widgets::StatusTone::Info,
            "PCSX2 widescreen and other PNACH patch directories can be inspected above. Preview, installation, enabling, disabling, replacement, and rollback are unavailable.",
        )
    } else if dolphin_read_only {
        (
            widgets::StatusTone::Info,
            "Individual exact-ID Gecko codes from the external provider can be selected, applied, and rolled back above, and a single PNG hires-texture file can be installed and undone in the Dolphin texture mod panel above. Texture packs, Riivolution assets, and other Dolphin mod types remain unavailable.",
        )
    } else {
        (widgets::StatusTone::Pending, MODS_UNAVAILABLE_BODY)
    };
    widgets::banner(ui, "Mods: planned", detail, tone);
}


pub(crate) fn platform_is_ps2(platform: Option<&str>) -> bool {
    platform.is_some_and(|platform| platform.eq_ignore_ascii_case("PS2"))
}


pub(crate) fn platform_is_dolphin(platform: Option<&str>) -> bool {
    platform.is_some_and(|platform| {
        ["GameCube", "Nintendo GameCube", "Wii", "Nintendo Wii"]
            .iter()
            .any(|candidate| platform.eq_ignore_ascii_case(candidate))
    })
}


pub(crate) fn detected_platform_counts<'a>(
    platforms: impl Iterator<Item = Option<&'a str>>,
) -> DetectedPlatformCounts {
    let mut counts: std::collections::BTreeMap<&'a str, usize> = std::collections::BTreeMap::new();
    let mut unknown = 0_usize;
    for platform in platforms {
        match platform {
            Some(platform) => *counts.entry(platform).or_default() += 1,
            None => unknown += 1,
        }
    }
    DetectedPlatformCounts {
        named: counts
            .into_iter()
            .map(|(platform, count)| (platform.to_string(), count))
            .collect(),
        unknown,
    }
}


pub(crate) fn platform_is_gamecube(platform: Option<&str>) -> bool {
    platform.is_some_and(|platform| {
        ["GameCube", "Nintendo GameCube"]
            .iter()
            .any(|candidate| platform.eq_ignore_ascii_case(candidate))
    })
}


/// Whether `poll_cheat_workflow` should quietly start a background
/// Dolphin Gecko-provider fetch this frame: identity is ready, nothing
/// has been requested yet (`NotLoaded`), and the adapter/platform are
/// actually Dolphin/GameCube. `NotLoaded` is a one-shot gate - once a
/// fetch starts (`Loading`) or finishes (`Ready`/`Failed`), this returns
/// `false` again on every later poll, so a fixed page never keeps
/// re-triggering requests just because it keeps re-rendering.
pub(crate) fn dolphin_provider_auto_fetch_needed(workflow: &CheatWorkflowState) -> bool {
    workflow.adapter == CheatEmulatorAdapter::Dolphin
        && platform_is_gamecube(workflow.platform.as_deref())
        && ready_game_identity(workflow)
            .and_then(GameIdentityReport::verified_dolphin_game_id)
            .is_some()
        && matches!(workflow.dolphin_provider, CheatStepResource::NotLoaded)
}


pub(crate) fn wii_gamehacking_auto_match_needed(workflow: &CheatWorkflowState) -> bool {
    workflow.adapter == CheatEmulatorAdapter::Dolphin
        && workflow.platform.as_deref() == Some("Wii")
        && wii_identity_for_workflow(workflow)
            .and_then(|identity| identity.verified_game_id().map(str::to_owned))
            .is_some()
        && matches!(workflow.gamecube_gamehacking, CheatStepResource::NotLoaded)
        && workflow.gamecube_gamehacking_request.is_none()
}


/// Xenia's counterpart to `dolphin_provider_auto_fetch_needed`.
pub(crate) fn xenia_provider_auto_fetch_needed(workflow: &CheatWorkflowState) -> bool {
    workflow.adapter == CheatEmulatorAdapter::Xenia
        && matches!(workflow.identity, CheatStepResource::Ready(_))
        && matches!(workflow.xenia_provider, CheatStepResource::NotLoaded)
}


pub(crate) fn platform_is_xenia(platform: Option<&str>) -> bool {
    platform.is_some_and(|platform| {
        ["Xbox360", "Xbox 360"]
            .iter()
            .any(|candidate| platform.eq_ignore_ascii_case(candidate))
    })
}


/// Routes one canonical library platform to exactly one workflow. This is
/// intentionally not a UI preference: rendering two adapters against one
/// archive allowed stale profile/candidate state from the wrong system to
/// remain reachable.
pub(crate) fn cheat_adapter_route(platform: Option<&str>) -> CheatEmulatorAdapter {
    if platform_is_ps2(platform) {
        CheatEmulatorAdapter::Pcsx2
    } else if platform_is_dolphin(platform) {
        CheatEmulatorAdapter::Dolphin
    } else if platform_is_xenia(platform) {
        CheatEmulatorAdapter::Xenia
    } else if platform.is_some_and(|platform| {
        !platform.trim().is_empty() && !platform.eq_ignore_ascii_case("unknown")
    }) {
        CheatEmulatorAdapter::RetroArch
    } else {
        CheatEmulatorAdapter::Unsupported
    }
}


pub(crate) fn show_pcsx2_workflow(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    profiles: &Pcsx2ProfilesState,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    let cheats_directory = workflow
        .selected_pcsx2_profile_id
        .as_deref()
        .and_then(|profile_id| resolved_pcsx2_cheats_directory(profiles, profile_id));
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong("PCSX2 cheat setup");
            let (label, tone) = match profiles {
                Pcsx2ProfilesState::NotScanned => ("Not checked", widgets::StatusTone::Pending),
                Pcsx2ProfilesState::Scanning { .. } => ("Checking…", widgets::StatusTone::Active),
                Pcsx2ProfilesState::Error(_) => ("Setup incomplete", widgets::StatusTone::Blocked),
                Pcsx2ProfilesState::Ready(discovery) if discovery.profiles.is_empty() => {
                    ("Not found", widgets::StatusTone::Pending)
                }
                Pcsx2ProfilesState::Ready(_) if cheats_directory.is_some() => {
                    ("Setup selected", widgets::StatusTone::Success)
                }
                Pcsx2ProfilesState::Ready(_) => ("Choose a setup", widgets::StatusTone::Pending),
            };
            widgets::status_badge(ui, label, tone);
        });
        ui.label(if cheats_directory.is_some() {
            "PCSX2 is selected. EmuWiz can now check compatible cheats for this game."
        } else {
            "Choose a usable PCSX2 setup below before installing cheats."
        });
        show_cheat_activation_status(ui, "PCSX2", workflow.pcsx2_activation);
    });
    widgets::section_header(
        ui,
        "PCSX2 setup details",
        Some("EmuWiz selects automatically only when exactly one discovered profile is eligible."),
    );
    match profiles {
        Pcsx2ProfilesState::NotScanned => {
            widgets::banner(
                ui,
                "Profiles not scanned",
                "Run local read-only discovery of documented PCSX2 configuration paths.",
                widgets::StatusTone::Pending,
            );
        }
        Pcsx2ProfilesState::Scanning { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Scanning PCSX2 profiles locally...");
            });
        }
        Pcsx2ProfilesState::Error(message) => {
            widgets::banner(
                ui,
                "PCSX2 discovery failed",
                message,
                widgets::StatusTone::Blocked,
            );
        }
        Pcsx2ProfilesState::Ready(discovery) => {
            let eligible = eligible_pcsx2_profile_ids(discovery);
            if let Some(selected) = workflow.selected_pcsx2_profile_id.clone()
                && !eligible.contains(&selected.as_str())
            {
                workflow.selected_pcsx2_profile_id = None;
                workflow.pcsx2_inventory_profile_id = None;
                workflow.pcsx2_inventory = CheatStepResource::NotLoaded;
                workflow.pcsx2_activation = CheatActivationReadiness::Unknown;
                workflow.pcsx2_activation_receiver = None;
            }
            if discovery.profiles.is_empty() {
                widgets::banner(
                    ui,
                    "No PCSX2 profile found",
                    "No documented PCSX2 configuration directory was discovered. Missing cheat directories are not created.",
                    widgets::StatusTone::Pending,
                );
            } else if eligible.len() > 1 && workflow.selected_pcsx2_profile_id.is_none() {
                ui.label(format!(
                    "{} eligible profiles were found. Choose one explicitly.",
                    eligible.len()
                ));
            }
            for profile in &discovery.profiles {
                show_pcsx2_profile_card(ui, workflow, profile, clipboard);
                ui.add_space(6.0);
            }
            for warning in &discovery.warnings {
                widgets::banner(
                    ui,
                    "Discovery limit",
                    &warning.detail,
                    widgets::StatusTone::Warning,
                );
            }
            if workflow.selected_pcsx2_profile_id.is_some() {
                match &cheats_directory {
                    Some(cheats_directory) => {
                        widgets::banner(
                            ui,
                            "PCSX2 setup selected",
                            "EmuWiz found the folder PCSX2 uses for cheats.",
                            widgets::StatusTone::Success,
                        );
                        widgets::technical_details(ui, "pcsx2_cheats_directory", |ui| {
                            ui.label(format!(
                                "Cheats will be installed to: {}",
                                cheats_directory.display()
                            ));
                        });
                    }
                    None => {
                        widgets::banner(
                            ui,
                            "PCSX2 cheats directory could not be confidently identified",
                            "EmuWiz will not guess a cheats directory for this profile. Choose a different profile, or resolve the profile's blockers above, before installing.",
                            widgets::StatusTone::Blocked,
                        );
                    }
                }
            }
        }
    }
    if widgets::action_button(
        ui,
        "Rescan PCSX2 profiles",
        widgets::ActionStyle::Quiet,
        !matches!(profiles, Pcsx2ProfilesState::Scanning { .. }),
    )
    .clicked()
    {
        action = Some(CheatWorkflowAction::RescanPcsx2Profiles);
    }

    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(
        ui,
        "Existing PCSX2 files",
        Some(
            "Cheats, widescreen patches, and other PNACH categories are inferred only from documented directory locations.",
        ),
    );
    widgets::card(ui, |ui| {
        ui.label("This check stays on your computer and does not run or change cheat files.");
        widgets::technical_details(ui, "pcsx2_local_inspection_safety", |ui| {
            widgets::status_strip(
                ui,
                &[
                    ("Unverified local content", widgets::StatusTone::Warning),
                    ("Read-only", widgets::StatusTone::Success),
                    ("Uploaded · No", widgets::StatusTone::Info),
                    ("Executed · No", widgets::StatusTone::Info),
                    ("Changed · No", widgets::StatusTone::Info),
                ],
            );
            ui.label("EmuWiz inspects PNACH structure locally. It never invokes PCSX2, evaluates directives, or claims that structural inspection proves content is malware-free.");
        });
    });
    let Some(selected_profile_id) = workflow.selected_pcsx2_profile_id.as_deref() else {
        widgets::banner(
            ui,
            "Waiting for profile",
            "Choose an eligible PCSX2 profile before inspecting existing files.",
            widgets::StatusTone::Pending,
        );
        return show_pcsx2_gamehacking(ui, workflow, cheats_directory.as_deref()).or(action);
    };
    if workflow.pcsx2_inventory_profile_id.as_deref() != Some(selected_profile_id) {
        workflow.pcsx2_inventory_profile_id = None;
        workflow.pcsx2_inventory = CheatStepResource::NotLoaded;
    }
    match &workflow.pcsx2_inventory {
        CheatStepResource::NotLoaded => {
            if widgets::action_button(
                ui,
                "Inspect existing PNACH files",
                widgets::ActionStyle::Primary,
                true,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::InspectPcsx2Profile);
            }
        }
        CheatStepResource::Loading { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Inspecting existing PNACH files locally...");
            });
        }
        CheatStepResource::Failed(message) => {
            widgets::banner(
                ui,
                "PCSX2 inspection failed",
                message,
                widgets::StatusTone::Blocked,
            );
        }
        CheatStepResource::Ready(inventory) => {
            show_pcsx2_inventory(ui, workflow, inventory, clipboard);
        }
    }
    show_pcsx2_gamehacking(ui, workflow, cheats_directory.as_deref()).or(action)
}


/// The beginner-facing entry point: a plain-English status, a simplified
/// compatible-candidate checklist, one-click install, and Undo, with
/// every technical control from the previous numbered-stage page moved
/// into a collapsed-by-default "Details" disclosure below. Nothing here
/// is a new safety mechanism - it only reads and drives the same
/// workflow state and dispatch actions `show_dolphin_workflow_details`
/// already used.
pub(crate) fn show_dolphin_workflow(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    profiles: &DolphinProfilesState,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<CheatWorkflowAction> {
    let mut action = show_dolphin_beginner_summary(ui, workflow, profiles);
    ui.add_space(theme::SECTION_GAP);
    let details_response = egui::CollapsingHeader::new("Details")
        .id_salt("dolphin_workflow_details")
        .open(Some(workflow.dolphin_details_open))
        .show(ui, |ui| {
            show_dolphin_workflow_details(ui, workflow, profiles, clipboard)
        });
    if details_response.header_response.clicked() {
        action = Some(CheatWorkflowAction::ToggleDolphinDetailsOpen(
            !workflow.dolphin_details_open,
        ))
        .or(action);
    }
    if let Some(Some(details_action)) = details_response.body_returned {
        action = Some(details_action).or(action);
    }
    if matches!(workflow.platform.as_deref(), Some("GameCube" | "Wii")) {
        action = show_gamecube_gamehacking(ui, workflow).or(action);
    }
    if workflow.platform.as_deref() == Some("GameCube") {
        action = show_bsfree_gamecube(ui, workflow).or(action);
    }
    if workflow.platform.as_deref() == Some("Wii") {
        action = show_bsfree_wii(ui, workflow).or(action);
    }
    action
}


/// Renders the plain-English status, the compatible-candidate checklist,
/// the profile chooser (when one is needed), the one-click "Install
/// selected" flow, and the installed/Undo state. Everything it reads
/// (`dolphin_profile_selection`, `dolphin_provider`,
/// `dolphin_provider_selection`, `transaction`) is exactly what the
/// technical Details view already reads - this is a presentation layer,
/// not a second source of truth.
pub(crate) fn show_dolphin_beginner_summary(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    profiles: &DolphinProfilesState,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    if !bsfree_transaction_active(workflow) {
        match &workflow.transaction {
            CheatTransactionState::Applying { .. } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Installing…");
                });
                return action;
            }
            CheatTransactionState::Result { result, .. } => {
                return show_beginner_install_result(ui, result);
            }
            CheatTransactionState::Idle | CheatTransactionState::Review { .. } => {}
        }
    }
    let status = dolphin_beginner_status(workflow);
    widgets::status_badge(ui, status.label(), status.tone());
    show_cheat_activation_status(ui, "Dolphin", workflow.dolphin_activation);
    match &status {
        BeginnerCheatStatus::CouldNotCheckForCheats { .. } => {
            ui.label(
                "EmuWiz could not load compatible cheats. Check your connection and try again.",
            );
            if widgets::action_button(ui, "Try again", widgets::ActionStyle::Secondary, true)
                .clicked()
            {
                action = Some(CheatWorkflowAction::FetchDolphinProvider {
                    force_refresh: false,
                });
            }
        }
        BeginnerCheatStatus::NoUpstreamCheatsAvailable => {
            ui.label(
                "Dolphin's upstream GameSettings dataset has no entry for this exact game. This is expected for many games and is not an error - there is nothing to retry.",
            );
        }
        BeginnerCheatStatus::EmulatorSetupNeeded => {
            ui.label(
                "EmuWiz could not resolve one Dolphin profile safely. Open Details to select a discovered profile or provide a custom user directory.",
            );
        }
        BeginnerCheatStatus::FindingCompatibleCheats => {
            ui.horizontal(|ui| {
                ui.spinner();
            });
        }
        _ => {}
    }
    if status == BeginnerCheatStatus::ChooseEmulatorProfile
        && let DolphinProfilesState::Ready(discovery) = profiles
    {
        ui.add_space(theme::SECTION_GAP);
        return show_dolphin_profile_chooser(ui, workflow, discovery).or(action);
    }
    let Some(state) = workflow.dolphin_provider_selection.clone() else {
        return action;
    };
    let visible_indices: Vec<usize> = state
        .selection
        .entries
        .iter()
        .filter(|entry| entry.selectable)
        .map(|entry| entry.index)
        .collect();
    if visible_indices.len() > 1 {
        ui.horizontal(|ui| {
            let selected_count = state.selection.selected_count();
            if widgets::action_button(
                ui,
                "Select all compatible",
                widgets::ActionStyle::Secondary,
                selected_count < visible_indices.len(),
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::SelectAllDolphinCodes);
            }
            if widgets::action_button(
                ui,
                "Clear selection",
                widgets::ActionStyle::Quiet,
                selected_count > 0,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::ClearAllDolphinCodes);
            }
        });
    }
    for entry in state
        .selection
        .entries
        .iter()
        .filter(|entry| entry.selectable)
    {
        widgets::card(ui, |ui| {
            let mut selected = entry.selected;
            ui.horizontal_wrapped(|ui| {
                if ui.checkbox(&mut selected, &entry.name).changed() {
                    action = Some(CheatWorkflowAction::ToggleDolphinCodeSelected {
                        index: entry.index,
                        selected,
                    });
                }
                if entry.uncertain_revision {
                    widgets::status_badge(ui, "Probably compatible", widgets::StatusTone::Warning);
                } else {
                    widgets::status_badge(ui, "Compatible", widgets::StatusTone::Success);
                }
                if entry.already_present && entry.already_enabled {
                    widgets::status_badge(ui, "Installed", widgets::StatusTone::Info);
                }
            });
            for note in &entry.notes {
                ui.label(note);
            }
        });
    }
    let selected_entries: Vec<_> = state
        .selection
        .entries
        .iter()
        .filter(|entry| entry.selected)
        .collect();
    let has_pending_change = selected_entries
        .iter()
        .any(|entry| !(entry.already_present && entry.already_enabled));
    ui.add_space(theme::SECTION_GAP);
    match &mut workflow.transaction {
        CheatTransactionState::Idle => {
            if !selected_entries.is_empty() && !has_pending_change {
                ui.label("Already installed - no change to apply.");
                let _ = widgets::action_button(
                    ui,
                    "Install selected",
                    widgets::ActionStyle::Primary,
                    false,
                );
            } else if widgets::action_button(
                ui,
                "Install selected",
                widgets::ActionStyle::Primary,
                !selected_entries.is_empty() && has_pending_change,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::InstallSelectedDolphin);
            }
        }
        CheatTransactionState::Review {
            plan,
            replacement_approved,
            ..
        } => {
            action = show_beginner_install_confirm(
                ui,
                "Dolphin",
                plan,
                replacement_approved,
                workflow.dolphin_show_exact_changes,
                CheatWorkflowAction::ToggleDolphinShowExactChanges(
                    !workflow.dolphin_show_exact_changes,
                ),
                CheatWorkflowAction::ConfirmApply,
                CheatWorkflowAction::CancelApply,
            )
            .or(action);
        }
        CheatTransactionState::Applying { .. } | CheatTransactionState::Result { .. } => {
            unreachable!("handled before rendering the selectable list")
        }
    }
    action
}


/// The profile chooser: shown only while `select_emulator_profile` has
/// found more than one valid Dolphin profile and nothing has been
/// remembered or explicitly chosen yet. One concise choice, remembered
/// for next time - never shown again for this emulator once a choice is
/// confirmed and nothing invalidates it.
pub(crate) fn show_dolphin_profile_chooser(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    discovery: &DolphinProfileDiscovery,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    let eligible: Vec<&DolphinProfile> = discovery.profiles.iter().filter(|p| p.eligible).collect();
    widgets::card(ui, |ui| {
        ui.strong("Select the Dolphin profile to use");
        ui.label(format!(
            "EmuWiz found {} credible Dolphin profile{}.",
            eligible.len(),
            if eligible.len() == 1 { "" } else { "s" }
        ));
        for (index, profile) in eligible.iter().enumerate() {
            let selected =
                workflow.dolphin_profile_choice.as_deref() == Some(profile.profile_id.as_str());
            let same_kind_count = eligible
                .iter()
                .filter(|candidate| candidate.installation_type == profile.installation_type)
                .count();
            let label = if same_kind_count > 1 {
                format!(
                    "{} profile {}",
                    dolphin_installation_label(profile.installation_type),
                    index + 1
                )
            } else {
                format!(
                    "{} profile",
                    dolphin_installation_label(profile.installation_type)
                )
            };
            if ui.radio(selected, label).clicked() {
                action = Some(CheatWorkflowAction::ChooseDolphinProfile(
                    profile.profile_id.clone(),
                ));
            }
            ui.label(format!(
                "User root: {}",
                profile.configuration_path.display()
            ));
            ui.label(format!(
                "GameSettings destination: {}",
                profile.game_settings_path.display()
            ));
            ui.weak(format!(
                "Evidence: {}",
                profile.resolved.discovery_evidence.join("; ")
            ));
        }
        ui.weak("No profile is chosen merely because it appeared first. Your explicit choice will be remembered.");
    });
    action
}


/// The beginner confirmation dialog shown once "Install selected" has
/// built and moved to the review stage - shared verbatim by Dolphin and
/// Xenia since both use the same adapter-agnostic `SharedTransactionPlan`.
/// `replacement_approved` and `show_exact_changes` are both owned by the
/// caller's `CheatWorkflowState`, not duplicated here.
#[allow(clippy::too_many_arguments)]
pub(crate) fn show_beginner_install_confirm(
    ui: &mut egui::Ui,
    emulator_name: &str,
    plan: &SharedTransactionPlan,
    replacement_approved: &mut bool,
    show_exact_changes: bool,
    toggle_show_exact_changes: CheatWorkflowAction,
    confirm_action: CheatWorkflowAction,
    cancel_action: CheatWorkflowAction,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    let replacement_required = plan.entries.iter().any(|entry| {
        entry.proposed_action == archivefs_core::patch_manager::PreviewProposedAction::Replace
    });
    widgets::card(ui, |ui| {
        let count = plan.entries.len();
        let noun = if count == 1 {
            "enhancement"
        } else {
            "enhancements"
        };
        ui.strong(format!("Install {count} {noun} in {emulator_name}?"));
        ui.label("EmuWiz will back up the existing settings and you can undo this change later.");
        if replacement_required {
            ui.checkbox(
                replacement_approved,
                "I approve replacing the exact different file shown under Show exact changes",
            );
        }
        ui.horizontal_wrapped(|ui| {
            if widgets::action_button(
                ui,
                "Install",
                widgets::ActionStyle::Primary,
                !replacement_required || *replacement_approved,
            )
            .clicked()
            {
                action = Some(confirm_action.clone());
            }
            if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true).clicked() {
                action = Some(cancel_action.clone());
            }
            if widgets::action_button(ui, "Show exact changes", widgets::ActionStyle::Quiet, true)
                .clicked()
            {
                action = Some(toggle_show_exact_changes.clone());
            }
        });
        if show_exact_changes {
            ui.separator();
            widgets::copyable_value(ui, "Plan ID", &plan.plan_id);
            for entry in &plan.entries {
                ui.label(format!("Action: {:?}", entry.proposed_action));
                ui.label(format!("Source: {}", entry.source_path.display));
                ui.label(format!(
                    "Destination: {}/{}",
                    entry.destination_root.display, entry.destination_relative_path.display
                ));
                widgets::copyable_value(ui, "Source SHA-256", &entry.source_digest);
            }
        }
    });
    action
}


/// The beginner "installed"/Undo state shown once a beginner install has
/// been applied - shared by Dolphin and Xenia for the same reason as
/// `show_beginner_install_confirm`.
pub(crate) fn show_beginner_install_result(
    ui: &mut egui::Ui,
    result: &SharedApplyResult,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    widgets::card(ui, |ui| {
        match result.journal.status {
            SharedApplyStatus::Success => {
                widgets::status_badge(ui, "Installed successfully", widgets::StatusTone::Success);
            }
            SharedApplyStatus::PartialFailure => {
                widgets::status_badge(
                    ui,
                    "Installed with some problems",
                    widgets::StatusTone::Warning,
                );
            }
            SharedApplyStatus::Failed => {
                widgets::status_badge(ui, "Install failed", widgets::StatusTone::Blocked);
            }
            SharedApplyStatus::DryRun => {
                widgets::status_badge(ui, "Dry run complete", widgets::StatusTone::Info);
            }
        }
        for entry in &result.journal.entries {
            ui.label(format!(
                "Live target: {}/{}",
                entry.plan_entry.destination_root.display,
                entry.plan_entry.destination_relative_path.display
            ));
            for failure in &entry.failures {
                ui.label(format!(
                    "Failed stage: {:?} · target: {} · {}",
                    failure.kind,
                    failure
                        .path
                        .as_ref()
                        .map(|path| path.display.as_str())
                        .unwrap_or("unknown"),
                    failure.detail
                ));
            }
        }
        let rollback_available = result.journal_path.is_some()
            && matches!(
                result.journal.status,
                SharedApplyStatus::Success | SharedApplyStatus::PartialFailure
            );
        if widgets::action_button(
            ui,
            "Undo installation",
            widgets::ActionStyle::Destructive,
            rollback_available,
        )
        .clicked()
        {
            action = Some(CheatWorkflowAction::RollbackInstall);
        }
    });
    action
}


pub(crate) fn show_dolphin_workflow_details(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    profiles: &DolphinProfilesState,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    widgets::section_header(
        ui,
        "Game identity",
        Some("Read directly from the bounded GameCube/Wii disc header."),
    );
    widgets::card(ui, |ui| {
        widgets::status_rows(
            ui,
            &[
                (
                    "Platform",
                    workflow.platform.as_deref().unwrap_or("Unknown"),
                    widgets::StatusTone::Info,
                ),
                (
                    "Game ID",
                    match dolphin_identity_row_state(workflow) {
                        DolphinIdentityRowState::Verified(id) => id,
                        DolphinIdentityRowState::Pending => "Waiting for verified identity",
                        DolphinIdentityRowState::Unavailable => "Exact Game ID unavailable",
                    },
                    match dolphin_identity_row_state(workflow) {
                        DolphinIdentityRowState::Verified(_) => widgets::StatusTone::Success,
                        DolphinIdentityRowState::Pending => widgets::StatusTone::Pending,
                        DolphinIdentityRowState::Unavailable => widgets::StatusTone::Blocked,
                    },
                ),
            ],
        );
        if let Some(revision) =
            ready_game_identity(workflow).and_then(GameIdentityReport::verified_dolphin_revision)
        {
            ui.label(format!("Revision: {revision}"));
        }
        if let Some(report) = ready_game_identity(workflow)
            && report.verified_dolphin_game_id().is_none()
        {
            ui.label(dolphin_identity_unavailable_detail(report));
        }
    });
    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(
        ui,
        "Stage 1 · Dolphin profile",
        Some(
            "A unique running Dolphin profile wins. Otherwise EmuWiz selects automatically only when exactly one credible profile exists.",
        ),
    );
    match profiles {
        DolphinProfilesState::NotScanned => widgets::banner(
            ui,
            "Profiles not scanned",
            "Run local read-only discovery of documented Dolphin user directories.",
            widgets::StatusTone::Pending,
        ),
        DolphinProfilesState::Scanning { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Scanning Dolphin profiles locally...");
            });
        }
        DolphinProfilesState::Error(message) => widgets::banner(
            ui,
            "Dolphin discovery failed",
            message,
            widgets::StatusTone::Blocked,
        ),
        DolphinProfilesState::Ready(discovery) => {
            let eligible = eligible_dolphin_profile_ids(discovery);
            if let Some(selected) = workflow.selected_dolphin_profile_id.clone()
                && !eligible.contains(&selected.as_str())
            {
                workflow.selected_dolphin_profile_id = None;
                workflow.dolphin_inventory_profile_id = None;
                workflow.dolphin_inventory = CheatStepResource::NotLoaded;
            }
            if discovery.profiles.is_empty() {
                widgets::banner(
                    ui,
                    "No Dolphin profile found",
                    "No documented Dolphin user directory was discovered. Missing GameSettings directories are not created.",
                    widgets::StatusTone::Pending,
                );
            } else if eligible.len() > 1 && workflow.selected_dolphin_profile_id.is_none() {
                ui.label(format!(
                    "Select the Dolphin profile to use. {} credible profiles were found and no single active runtime resolved the choice.",
                    eligible.len()
                ));
            }
            for profile in &discovery.profiles {
                show_dolphin_profile_card(ui, workflow, profile, clipboard);
                ui.add_space(6.0);
            }
            for warning in &discovery.warnings {
                widgets::banner(
                    ui,
                    "Discovery limit",
                    &warning.detail,
                    widgets::StatusTone::Warning,
                );
            }
        }
    }
    ui.horizontal_wrapped(|ui| {
        ui.label("Additional Dolphin directory (portable/AppImage installs):");
        ui.text_edit_singleline(&mut workflow.dolphin_explicit_root);
    });
    ui.label(
        "Optional. Running Dolphin -u/--user directories, native profiles, and evidenced Flatpak profiles are discovered automatically. Enter a custom User directory only when it was not discovered.",
    );
    if widgets::action_button(
        ui,
        "Rescan Dolphin profiles",
        widgets::ActionStyle::Quiet,
        !matches!(profiles, DolphinProfilesState::Scanning { .. }),
    )
    .clicked()
    {
        action = Some(CheatWorkflowAction::RescanDolphinProfiles);
    }

    ensure_dolphin_provider_destination(workflow, profiles);

    ui.add_space(theme::SECTION_GAP);
    action = show_dolphin_external_provider(ui, workflow, clipboard).or(action);

    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(
        ui,
        "Stage 2 · Existing Dolphin-managed files",
        Some("GameSettings INI sections are parsed as bounded text and are never evaluated."),
    );
    widgets::card(ui, |ui| {
        widgets::status_strip(
            ui,
            &[
                ("Unverified local content", widgets::StatusTone::Warning),
                ("Read-only", widgets::StatusTone::Success),
                ("Uploaded · No", widgets::StatusTone::Info),
                ("Executed · No", widgets::StatusTone::Info),
                ("Changed · No", widgets::StatusTone::Info),
            ],
        );
        ui.label("EmuWiz inspects INI structure locally. It never invokes Dolphin, evaluates codes, follows referenced mod paths, or claims that structural inspection proves content is malware-free.");
    });
    let Some(selected_profile_id) = workflow.selected_dolphin_profile_id.as_deref() else {
        widgets::banner(
            ui,
            "Waiting for profile",
            "Choose an eligible Dolphin profile before inspecting existing files.",
            widgets::StatusTone::Pending,
        );
        show_dolphin_installation_unavailable(ui);
        return action;
    };
    if workflow.dolphin_inventory_profile_id.as_deref() != Some(selected_profile_id) {
        workflow.dolphin_inventory_profile_id = None;
        workflow.dolphin_inventory = CheatStepResource::NotLoaded;
    }
    match &workflow.dolphin_inventory {
        CheatStepResource::NotLoaded => {
            if widgets::action_button(
                ui,
                "Inspect existing Game INI files",
                widgets::ActionStyle::Primary,
                true,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::InspectDolphinProfile);
            }
        }
        CheatStepResource::Loading { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Inspecting GameSettings INI files locally...");
            });
        }
        CheatStepResource::Failed(message) => widgets::banner(
            ui,
            "Dolphin inspection failed",
            message,
            widgets::StatusTone::Blocked,
        ),
        CheatStepResource::Ready(inventory) => {
            show_dolphin_inventory(ui, workflow, inventory, clipboard)
        }
    }
    if workflow.dolphin_provider_selection.is_some() {
        ui.add_space(theme::SECTION_GAP);
        action = show_shared_cheat_preview(ui, workflow, clipboard).or(action);
    }
    action
}


pub(crate) fn ensure_dolphin_provider_destination(
    workflow: &mut CheatWorkflowState,
    profiles: &DolphinProfilesState,
) {
    let (Some(profile_id), CheatStepResource::Ready(fetch)) = (
        workflow.selected_dolphin_profile_id.as_deref(),
        &workflow.dolphin_provider,
    ) else {
        return;
    };
    let Some(configuration_path) = (match profiles {
        DolphinProfilesState::Ready(discovery) => discovery
            .profiles
            .iter()
            .find(|profile| profile.eligible && profile.profile_id == profile_id)
            .map(|profile| profile.configuration_path.clone()),
        _ => None,
    }) else {
        return;
    };
    let expected = configuration_path
        .join("GameSettings")
        .join(format!("{}.ini", fetch.result.game_id));
    if workflow
        .dolphin_provider_selection
        .as_ref()
        .is_some_and(|state| state.destination.path == expected)
    {
        return;
    }
    match load_dolphin_destination(&configuration_path, &fetch.result.game_id) {
        Ok(destination) => {
            let selection =
                DolphinProviderCodeSelection::from_provider(&fetch.result, &destination);
            workflow.dolphin_provider_selection = Some(DolphinProviderSelectionState {
                destination,
                selection,
            });
            workflow.dolphin_destination_error = None;
        }
        Err(error) => {
            workflow.dolphin_provider_selection = None;
            workflow.dolphin_destination_error = Some(error.to_string());
        }
    }
}


pub(crate) fn show_dolphin_external_provider(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    widgets::section_header(
        ui,
        "Stage 2 · Find matching cheats",
        Some("EmuWiz looks up the exact GameCube ID in Dolphin's upstream GameSettings dataset."),
    );
    widgets::card(ui, |ui| {
        widgets::status_rows(
            ui,
            &[
                (
                    "Provider",
                    "Dolphin upstream GameSettings",
                    widgets::StatusTone::Info,
                ),
                (
                    "Dataset",
                    "dolphin-emu/dolphin · GPL-2.0-or-later",
                    widgets::StatusTone::Info,
                ),
            ],
        );
        ui.label("Gecko definitions from the Dolphin Emulator upstream GameSettings dataset.");
    });
    if !platform_is_gamecube(workflow.platform.as_deref()) {
        widgets::banner(
            ui,
            "Cheats are not available for this game yet",
            "This milestone supports exact-ID external Gecko retrieval for GameCube only. Existing Wii GameSettings inspection remains read-only.",
            widgets::StatusTone::Pending,
        );
        return None;
    }
    let identity_ready = ready_game_identity(workflow)
        .and_then(GameIdentityReport::verified_dolphin_game_id)
        .is_some()
        && ready_game_identity(workflow)
            .and_then(GameIdentityReport::verified_dolphin_revision)
            .is_some();
    let profile_ready = workflow.selected_dolphin_profile_id.is_some();
    match &workflow.dolphin_provider {
        CheatStepResource::NotLoaded => {
            if widgets::action_button(
                ui,
                "Fetch Gecko codes",
                widgets::ActionStyle::Primary,
                identity_ready && profile_ready,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::FetchDolphinProvider {
                    force_refresh: false,
                });
            }
            if !identity_ready {
                ui.weak("Waiting for a verified GameCube game ID and disc revision.");
            } else if !profile_ready {
                ui.weak("Choose an eligible Dolphin profile before loading destination state.");
            }
        }
        CheatStepResource::Loading { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Retrieving exact-ID Gecko definitions...");
            });
        }
        CheatStepResource::Failed(message) => {
            widgets::banner(
                ui,
                "Could not load matching cheats",
                message,
                widgets::StatusTone::Blocked,
            );
            if widgets::action_button(
                ui,
                "Retry",
                widgets::ActionStyle::Primary,
                identity_ready && profile_ready,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::FetchDolphinProvider {
                    force_refresh: false,
                });
            }
        }
        CheatStepResource::Ready(fetch)
            if fetch.status == GeckoProviderFetchStatus::NotAvailable =>
        {
            widgets::banner(
                ui,
                "No upstream Dolphin cheats are available for this game.",
                "Dolphin's upstream GameSettings dataset has no file for this exact game ID — this is expected for many games and is not an error.",
                widgets::StatusTone::Info,
            );
        }
        CheatStepResource::Ready(fetch) => {
            widgets::card(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    widgets::status_badge(
                        ui,
                        dolphin_provider_fetch_status_label(fetch.status),
                        if fetch.status == GeckoProviderFetchStatus::StaleCacheFallback {
                            widgets::StatusTone::Warning
                        } else {
                            widgets::StatusTone::Success
                        },
                    );
                    ui.strong(format!(
                        "{} · {} · revision {}",
                        fetch.result.game_id,
                        fetch.result.region.display_name(),
                        fetch.result.revision
                    ));
                });
                if let Some(title) = &fetch.result.title {
                    ui.label(title);
                }
                widgets::copyable_value(ui, "Source", &fetch.result.source_identity);
                ui.label(format!(
                    "Retrieved: {}",
                    format_unix_timestamp_utc(fetch.result.retrieved_at_unix_seconds as i64)
                ));
                ui.label(format!("Licence: {}", fetch.result.license));
                ui.label(&fetch.result.attribution);
                for warning in &fetch.result.warnings {
                    widgets::banner(
                        ui,
                        "Provider warning",
                        warning,
                        widgets::StatusTone::Warning,
                    );
                }
                if let Some(error) = &fetch.refresh_error {
                    widgets::banner(
                        ui,
                        "Refresh unavailable; cached codes retained",
                        error,
                        widgets::StatusTone::Warning,
                    );
                }
                if widgets::action_button(
                    ui,
                    "Refresh",
                    widgets::ActionStyle::Quiet,
                    identity_ready && profile_ready,
                )
                .clicked()
                {
                    action = Some(CheatWorkflowAction::FetchDolphinProvider {
                        force_refresh: true,
                    });
                }
            });
        }
    }
    if let Some(message) = &workflow.dolphin_destination_error {
        widgets::banner(
            ui,
            "Dolphin destination unavailable",
            message,
            widgets::StatusTone::Blocked,
        );
    }
    ui.add_space(theme::SECTION_GAP);
    action = show_dolphin_provider_code_picker(ui, workflow, clipboard).or(action);
    action
}


pub(crate) fn show_dolphin_provider_code_picker(
    ui: &mut egui::Ui,
    workflow: &CheatWorkflowState,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    let (CheatStepResource::Ready(fetch), Some(state)) = (
        &workflow.dolphin_provider,
        workflow.dolphin_provider_selection.as_ref(),
    ) else {
        return None;
    };
    if fetch.result.entries.is_empty() {
        return None;
    }
    let selected_count = state.selection.selected_count();
    let selectable_count = state.selection.selectable_count();
    widgets::section_header(
        ui,
        "Stage 3 · Codes to install",
        Some(
            "Select individual validated definitions. Revision uncertainty is shown and never hidden.",
        ),
    );
    if widgets::path_value(ui, "Exact destination", &state.destination.path) {
        let _ = clipboard.set_text(state.destination.path.display().to_string());
    }
    ui.label(if state.destination.existed {
        "The destination exists; unrelated sections and Gecko entries will be preserved."
    } else {
        "No existing GameSettings file is required; apply will create this exact destination."
    });
    ui.horizontal_wrapped(|ui| {
        ui.strong(format!("{selected_count} of {selectable_count} selected"));
        if widgets::action_button(
            ui,
            "Select all",
            widgets::ActionStyle::Secondary,
            selected_count < selectable_count,
        )
        .clicked()
        {
            action = Some(CheatWorkflowAction::SelectAllDolphinCodes);
        }
        if widgets::action_button(
            ui,
            "Clear all",
            widgets::ActionStyle::Quiet,
            selected_count > 0,
        )
        .clicked()
        {
            action = Some(CheatWorkflowAction::ClearAllDolphinCodes);
        }
    });
    for entry in &state.selection.entries {
        widgets::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let mut selected = entry.selected;
                if ui
                    .add_enabled(
                        entry.selectable,
                        egui::Checkbox::new(&mut selected, &entry.name),
                    )
                    .changed()
                {
                    action = Some(CheatWorkflowAction::ToggleDolphinCodeSelected {
                        index: entry.index,
                        selected,
                    });
                }
                if entry.already_present {
                    widgets::status_badge(ui, "Already installed", widgets::StatusTone::Info);
                }
                if entry.already_enabled {
                    widgets::status_badge(ui, "Enabled", widgets::StatusTone::Success);
                }
                if entry.uncertain_revision {
                    widgets::status_badge(ui, "Revision uncertain", widgets::StatusTone::Warning);
                }
                if !entry.selectable {
                    widgets::status_badge(ui, "Blocked", widgets::StatusTone::Blocked);
                }
            });
            for note in &entry.notes {
                ui.label(note);
            }
            for warning in &entry.warnings {
                ui.weak(warning);
            }
            if let Some(provider_entry) = fetch.result.entries.get(entry.index) {
                widgets::technical_details(ui, ("provider_code", &entry.provider_entry_id), |ui| {
                    ui.code(provider_entry.code_lines.join("\n"));
                });
            }
        });
    }
    if widgets::action_button(
        ui,
        "Preview the installed file",
        widgets::ActionStyle::Primary,
        state.selection.can_preview(),
    )
    .clicked()
    {
        action = Some(CheatWorkflowAction::BuildDolphinInstallPreview);
    }
    action
}


pub(crate) fn xenia_compatibility_label(
    compatibility: XeniaCandidateCompatibility,
) -> (&'static str, widgets::StatusTone) {
    match compatibility {
        XeniaCandidateCompatibility::ExactCompatible => {
            ("Exact compatible", widgets::StatusTone::Success)
        }
        XeniaCandidateCompatibility::PartiallyVerified => {
            ("Partially verified", widgets::StatusTone::Warning)
        }
        XeniaCandidateCompatibility::Incompatible => ("Incompatible", widgets::StatusTone::Blocked),
    }
}


pub(crate) fn eligible_xenia_profile_ids(discovery: &XeniaProfileDiscovery) -> Vec<&str> {
    discovery
        .profiles
        .iter()
        .filter(|profile| profile.eligible)
        .map(|profile| profile.profile_id.as_str())
        .collect()
}


/// Maps Xenia's own discovery result into the adapter-agnostic shape
/// `select_emulator_profile` understands. Every Xenia profile EmuWiz
/// can discover is already an explicit, caller-supplied directory (there
/// is no single native Xenia Canary path to guess), so none of them are
/// singled out as "the portable one" - `is_portable` is left `false` for
/// all of them and the tie-break never fires for this adapter.
pub(crate) fn xenia_profile_candidates(discovery: &XeniaProfileDiscovery) -> Vec<EmulatorProfileCandidate> {
    discovery
        .profiles
        .iter()
        .map(|profile| EmulatorProfileCandidate {
            profile_id: profile.profile_id.clone(),
            root: profile.configuration_path.clone(),
            eligible: profile.eligible,
            is_portable: false,
            evidence_priority: 0,
        })
        .collect()
}


/// If exactly one candidate document was returned for this Title ID,
/// silently selects it - Xenia's provider dataset can legitimately have
/// several files per Title ID (different Title Update/module-hash
/// variants), so choosing between them is a real technical decision the
/// technical Details view still exposes explicitly via
/// `show_xenia_candidate_picker`/`show_xenia_external_provider`. This
/// only saves that step for the common case beginners actually hit: one
/// matching file. Never overrides a choice already made (including
/// `None` explicitly restored by "Choose a different file").
pub(crate) fn xenia_auto_select_single_candidate(workflow: &mut CheatWorkflowState) {
    if workflow.xenia_selected_candidate_index.is_some() {
        return;
    }
    let media_id =
        ready_game_identity(workflow).and_then(GameIdentityReport::verified_xex_media_id);
    let CheatStepResource::Ready(fetch) = &workflow.xenia_provider else {
        return;
    };
    let outcome = build_xenia_candidates(
        &fetch.result,
        Some(fetch.result.title_id.as_str()),
        media_id,
    );
    if outcome.candidates.len() == 1 {
        workflow.xenia_selected_candidate_index = Some(0);
    }
}


/// Beginner counterpart of `dolphin_beginner_status`/
/// `show_dolphin_beginner_summary` - see those for the shared design.
pub(crate) fn show_xenia_beginner_summary(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    profiles: &XeniaProfilesState,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    match &workflow.transaction {
        CheatTransactionState::Applying { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Installing…");
            });
            return action;
        }
        CheatTransactionState::Result { result, .. } => {
            return show_beginner_install_result(ui, result);
        }
        CheatTransactionState::Idle | CheatTransactionState::Review { .. } => {}
    }
    let status = xenia_beginner_status(workflow);
    widgets::status_badge(ui, status.label(), status.tone());
    show_cheat_activation_status(ui, "Xenia", CheatActivationReadiness::Unknown);
    match &status {
        BeginnerCheatStatus::CouldNotCheckForCheats { .. } => {
            ui.label(
                "EmuWiz could not load compatible patches. Check your connection and try again.",
            );
            if widgets::action_button(ui, "Try again", widgets::ActionStyle::Secondary, true)
                .clicked()
            {
                action = Some(CheatWorkflowAction::FetchXeniaProvider {
                    force_refresh: false,
                });
            }
        }
        BeginnerCheatStatus::EmulatorSetupNeeded => {
            ui.label(
                "EmuWiz could not find a Xenia Canary installation to use yet. Open Details to type the Xenia Canary directory.",
            );
        }
        BeginnerCheatStatus::FindingCompatibleCheats => {
            ui.horizontal(|ui| {
                ui.spinner();
            });
        }
        _ => {}
    }
    if status == BeginnerCheatStatus::ChooseEmulatorProfile
        && let XeniaProfilesState::Ready(discovery) = profiles
    {
        ui.add_space(theme::SECTION_GAP);
        return show_xenia_profile_chooser(ui, workflow, discovery).or(action);
    }
    xenia_auto_select_single_candidate(workflow);
    ensure_xenia_selection_state(workflow, profiles);
    let Some(state) = workflow.xenia_selection.clone() else {
        return action;
    };
    if state.selection.compatibility == XeniaCandidateCompatibility::PartiallyVerified {
        widgets::banner(
            ui,
            "More information needed",
            "This patch matches the game, but EmuWiz cannot confirm the exact executable version.",
            widgets::StatusTone::Warning,
        );
        let mut acknowledged = state.selection.partial_verification_acknowledged;
        if ui
            .checkbox(
                &mut acknowledged,
                "I understand this patch may target a different executable version.",
            )
            .changed()
        {
            action = Some(CheatWorkflowAction::AcknowledgeXeniaPartialVerification(
                acknowledged,
            ));
        }
    }
    let visible_count = state
        .selection
        .entries
        .iter()
        .filter(|entry| entry.selectable)
        .count();
    if visible_count > 1 {
        ui.horizontal(|ui| {
            let selected_count = state.selection.selected_count();
            if widgets::action_button(
                ui,
                "Select all compatible",
                widgets::ActionStyle::Secondary,
                selected_count < visible_count,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::SelectAllXeniaPatches);
            }
            if widgets::action_button(
                ui,
                "Clear selection",
                widgets::ActionStyle::Quiet,
                selected_count > 0,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::ClearAllXeniaPatches);
            }
        });
    }
    for entry in state
        .selection
        .entries
        .iter()
        .filter(|entry| entry.selectable)
    {
        widgets::card(ui, |ui| {
            let mut selected = entry.selected;
            ui.horizontal_wrapped(|ui| {
                if ui.checkbox(&mut selected, &entry.name).changed() {
                    action = Some(CheatWorkflowAction::ToggleXeniaPatchSelected {
                        index: entry.index,
                        selected,
                    });
                }
                match state.selection.compatibility {
                    XeniaCandidateCompatibility::ExactCompatible => {
                        widgets::status_badge(ui, "Compatible", widgets::StatusTone::Success);
                    }
                    XeniaCandidateCompatibility::PartiallyVerified => {
                        widgets::status_badge(
                            ui,
                            "Probably compatible",
                            widgets::StatusTone::Warning,
                        );
                    }
                    XeniaCandidateCompatibility::Incompatible => {}
                }
                if entry.already_enabled {
                    widgets::status_badge(ui, "Installed", widgets::StatusTone::Info);
                }
            });
            if !entry.description.is_empty() {
                ui.label(&entry.description);
            }
        });
    }
    ui.add_space(theme::SECTION_GAP);
    match &mut workflow.transaction {
        CheatTransactionState::Idle => {
            if widgets::action_button(
                ui,
                "Install selected",
                widgets::ActionStyle::Primary,
                state.selection.can_apply(),
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::InstallSelectedXenia);
            }
            if state.selection.selected_count() > 0 && !state.selection.can_apply() {
                ui.label(
                    "Acknowledge the warning above before installing this partially verified patch.",
                );
            }
        }
        CheatTransactionState::Review {
            plan,
            replacement_approved,
            ..
        } => {
            action = show_beginner_install_confirm(
                ui,
                "Xenia",
                plan,
                replacement_approved,
                workflow.xenia_show_exact_changes,
                CheatWorkflowAction::ToggleXeniaShowExactChanges(
                    !workflow.xenia_show_exact_changes,
                ),
                CheatWorkflowAction::ConfirmApply,
                CheatWorkflowAction::CancelApply,
            )
            .or(action);
        }
        CheatTransactionState::Applying { .. } | CheatTransactionState::Result { .. } => {
            unreachable!("handled before rendering the selectable list")
        }
    }
    action
}


/// Xenia's counterpart to `show_dolphin_profile_chooser`.
pub(crate) fn show_xenia_profile_chooser(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    discovery: &XeniaProfileDiscovery,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    let eligible: Vec<&XeniaProfile> = discovery.profiles.iter().filter(|p| p.eligible).collect();
    widgets::card(ui, |ui| {
        ui.strong(format!(
            "EmuWiz found {} Xenia Canary installations.",
            eligible.len()
        ));
        ui.label("Choose the one you use:");
        for (index, profile) in eligible.iter().enumerate() {
            let selected =
                workflow.xenia_profile_choice.as_deref() == Some(profile.profile_id.as_str());
            let folder_name = profile
                .configuration_path
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty());
            let label = folder_name.map_or_else(
                || format!("Xenia Canary installation {}", index + 1),
                |name| format!("Xenia Canary — {name}"),
            );
            if ui.radio(selected, label).clicked() {
                action = Some(CheatWorkflowAction::ChooseXeniaProfile(
                    profile.profile_id.clone(),
                ));
            }
        }
        ui.weak(
            "The chosen installation will be remembered. Exact folders are available in Details.",
        );
    });
    action
}


/// Route to Xbox 360/Xenia only: this page never shows Dolphin or
/// RetroArch controls, matching every other adapter's dedicated workflow.
pub(crate) fn show_xenia_workflow(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    profiles: &XeniaProfilesState,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<CheatWorkflowAction> {
    let mut action = show_xenia_beginner_summary(ui, workflow, profiles);
    ui.add_space(theme::SECTION_GAP);
    let details_response = egui::CollapsingHeader::new("Details")
        .id_salt("xenia_workflow_details")
        .open(Some(workflow.xenia_details_open))
        .show(ui, |ui| {
            show_xenia_workflow_details(ui, workflow, profiles, clipboard)
        });
    if details_response.header_response.clicked() {
        action = Some(CheatWorkflowAction::ToggleXeniaDetailsOpen(
            !workflow.xenia_details_open,
        ))
        .or(action);
    }
    if let Some(Some(details_action)) = details_response.body_returned {
        action = Some(details_action).or(action);
    }
    action
}


pub(crate) fn show_xenia_workflow_details(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    profiles: &XeniaProfilesState,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    widgets::section_header(
        ui,
        "Xbox 360 identity",
        Some("Read directly from the bounded, unencrypted XEX2 module header."),
    );
    widgets::card(ui, |ui| {
        widgets::status_rows(
            ui,
            &[
                (
                    "Platform",
                    workflow.platform.as_deref().unwrap_or("Unknown"),
                    widgets::StatusTone::Info,
                ),
                (
                    "Title ID",
                    ready_game_identity(workflow)
                        .and_then(GameIdentityReport::verified_xex_title_id)
                        .unwrap_or("Waiting for verified identity"),
                    if ready_game_identity(workflow)
                        .and_then(GameIdentityReport::verified_xex_title_id)
                        .is_some()
                    {
                        widgets::StatusTone::Success
                    } else {
                        widgets::StatusTone::Pending
                    },
                ),
            ],
        );
        if let Some(media_id) =
            ready_game_identity(workflow).and_then(GameIdentityReport::verified_xex_media_id)
        {
            ui.label(format!("Media ID: {media_id}"));
        }
        ui.label(
            "EmuWiz never computes or verifies a module hash - patches that require one are always shown as only partially verified.",
        );
    });

    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(
        ui,
        "Stage 1 · Xenia Canary profile",
        Some(
            "Xenia Canary has no single standard install location; supply its directory explicitly.",
        ),
    );
    match profiles {
        XeniaProfilesState::NotScanned => widgets::banner(
            ui,
            "Profile not checked yet",
            "Type the Xenia Canary directory below and rescan.",
            widgets::StatusTone::Pending,
        ),
        XeniaProfilesState::Ready(discovery) => {
            let eligible = eligible_xenia_profile_ids(discovery);
            if let Some(selected) = workflow.selected_xenia_profile_id.clone()
                && !eligible.contains(&selected.as_str())
            {
                workflow.selected_xenia_profile_id = None;
                workflow.xenia_selection = None;
            }
            if discovery.profiles.is_empty() {
                widgets::banner(
                    ui,
                    "No Xenia Canary directory checked yet",
                    "Type an explicit Xenia Canary directory below and rescan.",
                    widgets::StatusTone::Pending,
                );
            } else if eligible.len() > 1 && workflow.selected_xenia_profile_id.is_none() {
                ui.label(format!(
                    "{} eligible profiles were found. Choose one explicitly.",
                    eligible.len()
                ));
            }
            for profile in &discovery.profiles {
                show_xenia_profile_card(ui, workflow, profile, clipboard);
                ui.add_space(6.0);
            }
        }
    }
    ui.horizontal_wrapped(|ui| {
        ui.label("Xenia Canary directory:");
        ui.text_edit_singleline(&mut workflow.xenia_explicit_root);
    });
    ui.label(
        "The folder containing xenia_canary.exe and xenia-canary.config.toml (portable installs, including under Wine/Proton).",
    );
    if widgets::action_button(
        ui,
        "Rescan Xenia profiles",
        widgets::ActionStyle::Quiet,
        true,
    )
    .clicked()
    {
        action = Some(CheatWorkflowAction::RescanXeniaProfiles);
    }

    ensure_xenia_selection_state(workflow, profiles);

    ui.add_space(theme::SECTION_GAP);
    action = show_xenia_external_provider(ui, workflow, clipboard).or(action);

    if workflow.xenia_selection.is_some() {
        ui.add_space(theme::SECTION_GAP);
        action = show_shared_cheat_preview(ui, workflow, clipboard).or(action);
    }
    action
}


pub(crate) fn show_xenia_profile_card(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    profile: &XeniaProfile,
    clipboard: &mut dyn ClipboardBackend,
) {
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            if profile.eligible {
                let selected = workflow.selected_xenia_profile_id.as_deref()
                    == Some(profile.profile_id.as_str());
                if ui.radio(selected, &profile.profile_id).clicked() {
                    workflow.selected_xenia_profile_id = Some(profile.profile_id.clone());
                    workflow.xenia_selection = None;
                    workflow.xenia_destination_error = None;
                    workflow.preview_request = None;
                    workflow.preview = CheatStepResource::NotLoaded;
                    workflow.transaction = CheatTransactionState::Idle;
                }
            } else {
                widgets::status_badge(ui, "Blocked", widgets::StatusTone::Blocked);
                ui.strong(&profile.profile_id);
            }
        });
        if widgets::path_value(ui, "Configuration", &profile.configuration_path) {
            let _ = clipboard.set_text(profile.configuration_path.display().to_string());
        }
        ui.horizontal_wrapped(|ui| {
            let (label, tone) = match profile.patches_state {
                XeniaPatchesDirectoryState::Available => ("Exists", widgets::StatusTone::Success),
                XeniaPatchesDirectoryState::Missing => ("Missing", widgets::StatusTone::Pending),
                XeniaPatchesDirectoryState::UnsafePath => {
                    ("Unsafe path", widgets::StatusTone::Blocked)
                }
                XeniaPatchesDirectoryState::NotDirectory
                | XeniaPatchesDirectoryState::Unreadable => {
                    ("Unreadable", widgets::StatusTone::Warning)
                }
            };
            widgets::status_badge(ui, label, tone);
            if widgets::path_value(ui, "patches", &profile.patches_path) {
                let _ = clipboard.set_text(profile.patches_path.display().to_string());
            }
        });
        if let Some(warning) = &profile.patches_warning {
            ui.label(warning);
        }
        for blocker in &profile.blockers {
            ui.label(&blocker.detail);
        }
    });
}


/// Recomputes which real destination file the currently chosen candidate
/// (if any) would install to, and (re)loads it. Called every render, the
/// same way `ensure_dolphin_provider_destination` is - cheap, bounded,
/// local reads only.
pub(crate) fn ensure_xenia_selection_state(workflow: &mut CheatWorkflowState, profiles: &XeniaProfilesState) {
    let (Some(profile_id), CheatStepResource::Ready(fetch), Some(candidate_index)) = (
        workflow.selected_xenia_profile_id.as_deref(),
        &workflow.xenia_provider,
        workflow.xenia_selected_candidate_index,
    ) else {
        return;
    };
    let Some(configuration_path) = (match profiles {
        XeniaProfilesState::Ready(discovery) => discovery
            .profiles
            .iter()
            .find(|profile| profile.eligible && profile.profile_id == profile_id)
            .map(|profile| profile.configuration_path.clone()),
        XeniaProfilesState::NotScanned => None,
    }) else {
        return;
    };
    let outcome = build_xenia_candidates(
        &fetch.result,
        Some(fetch.result.title_id.as_str()),
        ready_game_identity(workflow).and_then(GameIdentityReport::verified_xex_media_id),
    );
    let Some(candidate) = outcome.candidates.get(candidate_index) else {
        workflow.xenia_selection = None;
        workflow.xenia_destination_error =
            Some("The chosen candidate is no longer present in the provider result.".to_string());
        return;
    };
    let Some(file_name) = Path::new(&candidate.source_path)
        .file_name()
        .and_then(|value| value.to_str())
    else {
        return;
    };
    let patches_directory = configuration_path.join("patches");
    let expected = patches_directory.join(file_name);
    if workflow
        .xenia_selection
        .as_ref()
        .is_some_and(|state| state.destination.path == expected)
    {
        return;
    }
    match load_xenia_destination(&patches_directory, file_name) {
        Ok(destination) => {
            let selection =
                XeniaPatchSelection::from_candidate(candidate, destination.document.as_ref());
            workflow.xenia_selection = Some(XeniaSelectionState {
                candidate: candidate.clone(),
                destination,
                selection,
            });
            workflow.xenia_destination_error = None;
        }
        Err(error) => {
            workflow.xenia_selection = None;
            workflow.xenia_destination_error = Some(error.to_string());
        }
    }
}


pub(crate) fn show_xenia_external_provider(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    widgets::section_header(
        ui,
        "Stage 2 · Find matching patches",
        Some(
            "EmuWiz looks up the exact Title ID in the xenia-canary/game-patches upstream dataset.",
        ),
    );
    widgets::card(ui, |ui| {
        widgets::status_rows(
            ui,
            &[(
                "Provider",
                XENIA_UPSTREAM_REPOSITORY,
                widgets::StatusTone::Info,
            )],
        );
        ui.label(XENIA_UPSTREAM_ATTRIBUTION);
        ui.label(XENIA_UPSTREAM_LICENSE);
    });
    let identity_ready = ready_game_identity(workflow)
        .and_then(GameIdentityReport::verified_xex_title_id)
        .is_some();
    let profile_ready = workflow.selected_xenia_profile_id.is_some();
    match &workflow.xenia_provider {
        CheatStepResource::NotLoaded => {
            if widgets::action_button(
                ui,
                "Fetch patches",
                widgets::ActionStyle::Primary,
                identity_ready && profile_ready,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::FetchXeniaProvider {
                    force_refresh: false,
                });
            }
            if !identity_ready {
                ui.weak("Waiting for a verified Xbox 360 Title ID.");
            } else if !profile_ready {
                ui.weak(
                    "Choose an eligible Xenia Canary profile before loading destination state.",
                );
            }
        }
        CheatStepResource::Loading { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Retrieving exact Title ID patches...");
            });
        }
        CheatStepResource::Failed(message) => {
            widgets::banner(
                ui,
                "Could not load matching patches",
                message,
                widgets::StatusTone::Blocked,
            );
            if widgets::action_button(
                ui,
                "Retry",
                widgets::ActionStyle::Primary,
                identity_ready && profile_ready,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::FetchXeniaProvider {
                    force_refresh: false,
                });
            }
        }
        CheatStepResource::Ready(fetch) => {
            widgets::card(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    widgets::status_badge(
                        ui,
                        match fetch.status {
                            XeniaProviderFetchStatus::Downloaded => "downloaded",
                            XeniaProviderFetchStatus::FreshCache => "fresh cache",
                            XeniaProviderFetchStatus::RateLimitedCache => "rate-limited cache",
                            XeniaProviderFetchStatus::StaleCacheFallback => "stale cache fallback",
                            XeniaProviderFetchStatus::OfflineCache => "offline cache",
                        },
                        if fetch.status == XeniaProviderFetchStatus::StaleCacheFallback {
                            widgets::StatusTone::Warning
                        } else {
                            widgets::StatusTone::Success
                        },
                    );
                    ui.strong(format!("Title ID {}", fetch.result.title_id));
                });
                widgets::copyable_value(ui, "Source revision", &fetch.result.source_commit);
                ui.label(format!(
                    "Retrieved: {}",
                    format_unix_timestamp_utc(fetch.result.retrieved_at_unix_seconds as i64)
                ));
                for warning in &fetch.result.warnings {
                    widgets::banner(
                        ui,
                        "Provider warning",
                        warning,
                        widgets::StatusTone::Warning,
                    );
                }
                if let Some(error) = &fetch.refresh_error {
                    widgets::banner(
                        ui,
                        "Refresh unavailable; cached patches retained",
                        error,
                        widgets::StatusTone::Warning,
                    );
                }
                if widgets::action_button(
                    ui,
                    "Refresh",
                    widgets::ActionStyle::Quiet,
                    identity_ready && profile_ready,
                )
                .clicked()
                {
                    action = Some(CheatWorkflowAction::FetchXeniaProvider {
                        force_refresh: true,
                    });
                }
            });
            let outcome = build_xenia_candidates(
                &fetch.result,
                Some(fetch.result.title_id.as_str()),
                ready_game_identity(workflow).and_then(GameIdentityReport::verified_xex_media_id),
            );
            action = show_xenia_candidate_picker(ui, workflow, &outcome, clipboard).or(action);
        }
    }
    if let Some(message) = &workflow.xenia_destination_error {
        widgets::banner(
            ui,
            "Xenia destination unavailable",
            message,
            widgets::StatusTone::Blocked,
        );
    }
    ui.add_space(theme::SECTION_GAP);
    action = show_xenia_patch_picker(ui, workflow, clipboard).or(action);
    action
}


/// Stage 2b: which of the (possibly several) returned candidate documents
/// to work with. Xenia's own dataset legitimately has multiple files per
/// Title ID (Title Update / module-hash variants); the user always
/// chooses explicitly, never an automatic pick.
pub(crate) fn show_xenia_candidate_picker(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    outcome: &XeniaCandidateOutcome,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    if let Some(reason) = outcome.blocked_reason {
        widgets::banner(
            ui,
            "No matching patches found",
            reason.message(),
            widgets::StatusTone::Pending,
        );
        return action;
    }
    if outcome.candidates.is_empty() {
        widgets::banner(
            ui,
            "No matching patches found",
            "The provider returned no patch files declaring this exact Title ID.",
            widgets::StatusTone::Pending,
        );
        return action;
    }
    widgets::section_header(
        ui,
        "Candidate files",
        Some(
            "Title similarity is never used - only exact Title ID, Media ID, and module-hash evidence.",
        ),
    );
    for (index, candidate) in outcome.candidates.iter().enumerate() {
        widgets::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let (label, tone) = xenia_compatibility_label(candidate.compatibility);
                widgets::status_badge(ui, label, tone);
                ui.strong(&candidate.title_name);
                let selected = workflow.xenia_selected_candidate_index == Some(index);
                if candidate.manually_selectable() {
                    if ui.radio(selected, "Choose this file").clicked() {
                        action = Some(CheatWorkflowAction::SelectXeniaCandidate(index));
                    }
                } else {
                    widgets::status_badge(ui, "Never selectable", widgets::StatusTone::Blocked);
                }
            });
            ui.label(&candidate.source_path);
            for evidence in &candidate.evidence {
                ui.horizontal_wrapped(|ui| {
                    widgets::status_badge(ui, evidence.label, widgets::StatusTone::Info);
                    ui.label(&evidence.detail);
                });
            }
            for warning in &candidate.document_warnings {
                ui.weak(warning);
            }
            let _ = clipboard;
        });
    }
    if workflow.xenia_selected_candidate_index.is_some()
        && widgets::action_button(
            ui,
            "Choose a different file",
            widgets::ActionStyle::Quiet,
            true,
        )
        .clicked()
    {
        action = Some(CheatWorkflowAction::ClearXeniaCandidateChoice);
    }
    action
}


/// Stage 3: the chosen candidate's own patches.
pub(crate) fn show_xenia_patch_picker(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    let state = workflow.xenia_selection.as_ref()?;
    widgets::section_header(
        ui,
        "Stage 3 · Patches to install",
        Some(
            "Nothing is selected by default. A partially verified file requires explicit acknowledgement below.",
        ),
    );
    if widgets::path_value(ui, "Exact destination", &state.destination.path) {
        let _ = clipboard.set_text(state.destination.path.display().to_string());
    }
    ui.label(if state.destination.existed {
        "The destination exists; unrelated patch definitions will be preserved."
    } else {
        "No existing patch file is required; apply will create this exact destination."
    });
    if state.selection.compatibility == XeniaCandidateCompatibility::PartiallyVerified {
        widgets::banner(
            ui,
            "Exact game version could not be confirmed",
            "This file's module hash cannot be computed or verified by EmuWiz. Acknowledge explicitly before any patch from it can be applied.",
            widgets::StatusTone::Warning,
        );
        let mut acknowledged = state.selection.partial_verification_acknowledged;
        if ui
            .checkbox(
                &mut acknowledged,
                "I understand the module hash is not verified and want to proceed",
            )
            .changed()
        {
            action = Some(CheatWorkflowAction::AcknowledgeXeniaPartialVerification(
                acknowledged,
            ));
        }
    }
    let selected_count = state.selection.selected_count();
    let selectable_count = state.selection.selectable_count();
    ui.horizontal_wrapped(|ui| {
        ui.strong(format!("{selected_count} of {selectable_count} selected"));
        if widgets::action_button(
            ui,
            "Select all",
            widgets::ActionStyle::Secondary,
            selected_count < selectable_count,
        )
        .clicked()
        {
            action = Some(CheatWorkflowAction::SelectAllXeniaPatches);
        }
        if widgets::action_button(
            ui,
            "Clear all",
            widgets::ActionStyle::Quiet,
            selected_count > 0,
        )
        .clicked()
        {
            action = Some(CheatWorkflowAction::ClearAllXeniaPatches);
        }
    });
    for entry in &state.selection.entries {
        widgets::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let mut selected = entry.selected;
                if ui
                    .add_enabled(
                        entry.selectable,
                        egui::Checkbox::new(&mut selected, &entry.name),
                    )
                    .changed()
                {
                    action = Some(CheatWorkflowAction::ToggleXeniaPatchSelected {
                        index: entry.index,
                        selected,
                    });
                }
                if entry.already_enabled {
                    widgets::status_badge(ui, "Already enabled in file", widgets::StatusTone::Info);
                }
                if !entry.selectable {
                    widgets::status_badge(ui, "Blocked", widgets::StatusTone::Blocked);
                }
            });
            if !entry.author.is_empty() {
                ui.label(format!("Author: {}", entry.author));
            }
            if !entry.description.is_empty() {
                ui.label(&entry.description);
            }
            for warning in &entry.warnings {
                ui.weak(warning);
            }
        });
    }
    if widgets::action_button(
        ui,
        "Preview the installed file",
        widgets::ActionStyle::Primary,
        state.selection.can_apply(),
    )
    .clicked()
    {
        action = Some(CheatWorkflowAction::BuildXeniaInstallPreview);
    }
    action
}


/// Stage 4: exactly what installing would write - the Xenia equivalent
/// of `show_generated_install_preview`/`show_dolphin_generated_install_preview`.
pub(crate) fn show_xenia_generated_install_preview(
    ui: &mut egui::Ui,
    generated: &GeneratedXeniaInstall,
    clipboard: &mut dyn ClipboardBackend,
) {
    widgets::card(ui, |ui| {
        widgets::status_badge(ui, "Preview only", widgets::StatusTone::Info);
        ui.strong(format!("Title ID: {}", generated.candidate.title_id));
        if widgets::path_value(ui, "Destination", &generated.destination) {
            let _ = clipboard.set_text(generated.destination.display().to_string());
        }
        ui.label(
            "This file already exists. Installing replaces it in place, preserving every patch definition not part of the chosen candidate, and the existing file is backed up first.",
        );
        ui.label(format!(
            "{} patch(es) selected.",
            generated.staged.selected_patch_count
        ));
        widgets::copyable_value(ui, "New file SHA-256", &generated.staged.digest);
        widgets::technical_details(ui, "generated_xenia_patch_toml_contents", |ui| {
            ui.label("Exact file contents:");
            ui.code(&generated.staged.contents);
        });
    });
}


/// Dolphin Stage 3/4: matches the verified game ID against the inspected
/// profile's own GameSettings files and, once matched, lets the user pick
/// which of that file's own Gecko codes to install.
#[cfg(any())]
pub(crate) fn show_dolphin_candidate_and_selection(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(
        ui,
        "Stage 3 · Matching Gecko codes",
        Some("Only an exact verified GameCube game ID and disc revision can produce a candidate."),
    );
    if widgets::action_button(
        ui,
        "Find matching Gecko codes",
        widgets::ActionStyle::Primary,
        true,
    )
    .clicked()
    {
        action = Some(CheatWorkflowAction::MatchDolphinCandidate);
    }
    match &workflow.dolphin_candidate_outcome {
        None => widgets::banner(
            ui,
            "Not matched yet",
            "Click \"Find matching Gecko codes\" to compare this archive's verified game ID against the selected profile's own GameSettings files.",
            widgets::StatusTone::Pending,
        ),
        Some(outcome) => match &outcome.candidate {
            Some(candidate) => show_dolphin_candidate_evidence(ui, candidate, clipboard),
            None => {
                let message = outcome
                    .blocked_reason
                    .map(DolphinCandidateBlockedReason::message)
                    .unwrap_or("No matching Game INI file was found.");
                widgets::banner(
                    ui,
                    "No exact candidate",
                    message,
                    widgets::StatusTone::Pending,
                );
                for path in &outcome.conflicting_paths {
                    if widgets::path_value(ui, "Conflicting file", path) {
                        let _ = clipboard.set_text(path.display().to_string());
                    }
                }
            }
        },
    }
    ui.add_space(theme::SECTION_GAP);
    action = show_dolphin_code_picker(ui, workflow).or(action);
    action
}


#[cfg(any())]
pub(crate) fn show_dolphin_candidate_evidence(
    ui: &mut egui::Ui,
    candidate: &DolphinCandidate,
    clipboard: &mut dyn ClipboardBackend,
) {
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            widgets::status_badge(ui, "Verified exact match", widgets::StatusTone::Success);
            ui.strong(&candidate.game_id);
            if let Some(revision) = candidate.revision {
                ui.label(format!("Revision {revision}"));
            }
        });
        if widgets::path_value(ui, "GameSettings file", &candidate.path) {
            let _ = clipboard.set_text(candidate.path.display().to_string());
        }
        ui.label(format!(
            "{} code(s) in this file · {} already enabled",
            candidate.cheat_count, candidate.enabled_count
        ));
        for evidence in &candidate.evidence {
            ui.horizontal_wrapped(|ui| {
                widgets::status_badge(ui, evidence.label, widgets::StatusTone::Info);
                ui.label(&evidence.detail);
            });
        }
    });
}


/// Dolphin Stage 4: the matched file's own Gecko codes, in file order.
#[cfg(any())]
pub(crate) fn show_dolphin_code_picker(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    let Some(state) = workflow.dolphin_selection.as_ref() else {
        return action;
    };
    let selected_count = state.selection.selected_count();
    let selectable_count = state.selection.selectable_count();
    let blocked_count = state.selection.entries.len() - selectable_count;

    widgets::section_header(
        ui,
        "Stage 4 · Codes to install",
        Some(
            "Ticked codes are written into [Gecko_Enabled]. Codes already enabled in the file start ticked - clearing one removes it on apply.",
        ),
    );
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("{selected_count} of {selectable_count} selected"));
            if blocked_count > 0 {
                widgets::status_badge(
                    ui,
                    format!("{blocked_count} unavailable"),
                    widgets::StatusTone::Warning,
                );
            }
            if widgets::action_button(
                ui,
                "Select all",
                widgets::ActionStyle::Secondary,
                selectable_count > 0 && selected_count < selectable_count,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::SelectAllDolphinCodes);
            }
            if widgets::action_button(
                ui,
                "Clear all",
                widgets::ActionStyle::Quiet,
                selected_count > 0,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::ClearAllDolphinCodes);
            }
        });
    });

    for entry in &state.selection.entries {
        widgets::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if entry.selectable {
                    let mut selected = entry.selected;
                    if ui.checkbox(&mut selected, &entry.name).changed() {
                        action = Some(CheatWorkflowAction::ToggleDolphinCodeSelected {
                            index: entry.index,
                            selected,
                        });
                    }
                } else {
                    ui.add_enabled(false, egui::Checkbox::new(&mut false, &entry.name));
                    widgets::status_badge(ui, "Unavailable", widgets::StatusTone::Blocked);
                }
                if entry.already_enabled {
                    widgets::status_badge(ui, "Already enabled in file", widgets::StatusTone::Info);
                }
            });
            for note in &entry.notes {
                ui.label(note);
            }
            for warning in &entry.warnings {
                ui.label(warning);
            }
        });
    }

    let Some(state) = workflow.dolphin_selection.as_ref() else {
        return action;
    };
    ui.add_space(theme::SECTION_GAP);
    widgets::card(ui, |ui| {
        if state.selection.can_apply() {
            if widgets::action_button(
                ui,
                "Preview the installed file",
                widgets::ActionStyle::Primary,
                !matches!(workflow.preview, CheatStepResource::Loading { .. }),
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::BuildDolphinInstallPreview);
            }
        } else {
            widgets::banner(
                ui,
                "Choose at least one code",
                "Nothing can be previewed or installed until at least one usable Gecko code is selected.",
                widgets::StatusTone::Pending,
            );
        }
    });
    action
}


/// Stage 5: exactly what installing would write, before anything is
/// written - the Dolphin equivalent of `show_generated_install_preview`.
pub(crate) fn show_dolphin_generated_install_preview(
    ui: &mut egui::Ui,
    generated: &GeneratedDolphinInstall,
    clipboard: &mut dyn ClipboardBackend,
) {
    widgets::card(ui, |ui| {
        widgets::status_badge(ui, "Preview only", widgets::StatusTone::Info);
        ui.strong(format!(
            "{} · Game ID {} · revision {}",
            generated.provider.result.provider_display_name,
            generated.provider.result.game_id,
            generated.provider.result.revision
        ));
        if widgets::path_value(ui, "Destination", &generated.destination) {
            let _ = clipboard.set_text(generated.destination.display().to_string());
        }
        ui.label(if generated.staged.destination_existed {
            "The existing file will be backed up. Unrelated Dolphin settings and unrelated Gecko definitions are preserved."
        } else {
            "The GameSettings file does not exist yet. Apply will create it; rollback will remove that newly-created file."
        });
        ui.label(format!(
            "{} code(s) selected for [Gecko_Enabled].",
            generated.staged.selected_code_count
        ));
        ui.label(format!(
            "Selected: {}",
            generated.staged.selected_code_names.join(", ")
        ));
        ui.label(format!(
            "Preserved existing sections: {}",
            if generated.staged.preserved_sections.is_empty() {
                "none (new file)".to_string()
            } else {
                generated.staged.preserved_sections.join(", ")
            }
        ));
        ui.label(format!(
            "Source: {} ({})",
            generated.provider.result.source_identity, generated.provider.result.license
        ));
        widgets::copyable_value(ui, "New file SHA-256", &generated.staged.digest);
        widgets::technical_details(ui, "generated_dolphin_ini_contents", |ui| {
            ui.label("Exact file contents:");
            ui.code(&generated.staged.contents);
        });
    });
}


pub(crate) fn show_dolphin_profile_card(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    profile: &DolphinProfile,
    clipboard: &mut dyn ClipboardBackend,
) {
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            if profile.eligible {
                let selected = workflow.selected_dolphin_profile_id.as_deref()
                    == Some(profile.profile_id.as_str());
                if ui.radio(selected, &profile.profile_id).clicked() {
                    workflow.selected_dolphin_profile_id = Some(profile.profile_id.clone());
                    workflow.dolphin_profile_choice = Some(profile.profile_id.clone());
                    workflow.dolphin_profile_selection = Some(EmulatorProfileSelection::Auto {
                        profile_id: profile.profile_id.clone(),
                        reason: EmulatorProfileSelectReason::ExplicitChoice,
                    });
                    workflow.dolphin_inventory_profile_id = None;
                    workflow.dolphin_inventory = CheatStepResource::NotLoaded;
                    workflow.dolphin_activation = CheatActivationReadiness::Unknown;
                    workflow.dolphin_activation_receiver = None;
                    workflow.dolphin_provider_selection = None;
                    workflow.dolphin_destination_error = None;
                    workflow.preview_request = None;
                    workflow.preview = CheatStepResource::NotLoaded;
                    workflow.transaction = CheatTransactionState::Idle;
                    bind_dolphin_provider_to_configuration(workflow, &profile.configuration_path);
                }
                if let Some(label) = dolphin_profile_selection_badge(workflow, profile) {
                    widgets::status_badge(ui, label, widgets::StatusTone::Success);
                }
            } else {
                widgets::status_badge(ui, "Blocked", widgets::StatusTone::Blocked);
                ui.strong(&profile.profile_id);
            }
            ui.label(format!(
                "Profile type: {} · {}",
                dolphin_installation_label(profile.installation_type),
                dolphin_scope_label(profile.scope)
            ));
        });
        if widgets::path_value(ui, "Dolphin user root", &profile.configuration_path) {
            let _ = clipboard.set_text(profile.configuration_path.display().to_string());
        }
        if let Some(executable) = &profile.resolved.emulator_executable
            && widgets::path_value(ui, "Executable", executable)
        {
            let _ = clipboard.set_text(executable.display().to_string());
        }
        ui.label(format!(
            "Discovery evidence: {}",
            profile.resolved.discovery_evidence.join("; ")
        ));
        ui.label(format!(
            "Confidence: {:?} · priority {} · writable: {}",
            profile.resolved.confidence,
            profile.resolved.priority,
            if profile.resolved.writable {
                "Yes"
            } else {
                "No"
            }
        ));
        ui.horizontal_wrapped(|ui| {
            widgets::status_badge(
                ui,
                dolphin_directory_state_label(profile.game_settings_state),
                dolphin_directory_state_tone(profile.game_settings_state),
            );
            if widgets::path_value(
                ui,
                "Exact GameSettings destination",
                &profile.game_settings_path,
            ) {
                let _ = clipboard.set_text(profile.game_settings_path.display().to_string());
            }
        });
        if let Some(warning) = &profile.game_settings_warning {
            ui.label(warning);
        }
        for blocker in &profile.blockers {
            ui.label(format!("{:?} — {}", blocker.kind, blocker.detail));
        }
    });
}


/// A checked radio button only says which destination the workflow is bound
/// to. It does not prove Dolphin is running. Runtime wording therefore needs
/// both the selection reason and the profile's verified runtime confidence.
pub(crate) fn dolphin_profile_selection_badge(
    workflow: &CheatWorkflowState,
    profile: &DolphinProfile,
) -> Option<&'static str> {
    if workflow.selected_dolphin_profile_id.as_deref() != Some(profile.profile_id.as_str()) {
        return None;
    }
    match workflow.dolphin_profile_selection.as_ref() {
        Some(EmulatorProfileSelection::Auto {
            profile_id,
            reason: EmulatorProfileSelectReason::StrongestEvidence,
        }) if profile_id == &profile.profile_id
            && profile.resolved.confidence
                == archivefs_core::patch_manager::EmulatorProfileConfidence::RunningExplicit =>
        {
            Some("Running Dolphin profile")
        }
        Some(EmulatorProfileSelection::Auto {
            profile_id,
            reason:
                EmulatorProfileSelectReason::ExplicitChoice | EmulatorProfileSelectReason::Remembered,
        }) if profile_id == &profile.profile_id => Some("Selected Dolphin profile"),
        _ => None,
    }
}


pub(crate) fn show_dolphin_inventory(
    ui: &mut egui::Ui,
    workflow: &CheatWorkflowState,
    inventory: &DolphinGameIniInventory,
    clipboard: &mut dyn ClipboardBackend,
) {
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            widgets::status_badge(
                ui,
                if inventory.complete {
                    "Complete"
                } else {
                    "Incomplete"
                },
                if inventory.complete {
                    widgets::StatusTone::Success
                } else {
                    widgets::StatusTone::Warning
                },
            );
            ui.strong(format!("{} Game INI files", inventory.files.len()));
            ui.label(format!("{} bytes inspected", inventory.bytes_inspected));
            ui.label(format!("{} entries visited", inventory.entries_visited));
        });
    });
    let identity = ready_game_identity(workflow);
    let match_result = match_dolphin_inventory(
        inventory,
        identity.and_then(GameIdentityReport::verified_dolphin_game_id),
        identity.and_then(GameIdentityReport::verified_dolphin_revision),
    );
    let (label, tone) = dolphin_match_presentation(match_result.state);
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            widgets::status_badge(ui, label, tone);
            ui.label(&match_result.reason);
        });
        ui.label("Only a verified disc-header Game ID can establish an exact match. Wii outer-header revision remains candidate evidence and cannot establish a revision-aware match.");
    });
    egui::CollapsingHeader::new(format!(
        "Inspected Game INI files ({})",
        inventory.files.len()
    ))
    .default_open(false)
    .show(ui, |ui| {
        const MAX_RENDERED: usize = 100;
        for file in inventory.files.iter().take(MAX_RENDERED) {
            widgets::card(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.strong(file.filename_stem.to_string_lossy());
                    if let Some(id) = &file.game_id_candidate {
                        widgets::status_badge(
                            ui,
                            format!("Game ID candidate · {id}"),
                            widgets::StatusTone::Pending,
                        );
                    }
                    if let Some(revision) = file.revision_candidate {
                        ui.label(format!("Revision {revision}"));
                    }
                });
                if widgets::path_value(ui, "INI", &file.path) {
                    let _ = clipboard.set_text(file.path.display().to_string());
                }
                ui.label(format!(
                    "Definitions {} · enabled references {}",
                    file.definition_count(),
                    file.enabled_count()
                ));
                ui.label(format!(
                    "Frame patches {} · Action Replay {} · Gecko {} · Riivolution {}",
                    file.frame_patch_names.len(),
                    file.action_replay_names.len(),
                    file.gecko_names.len(),
                    file.riivolution_names.len()
                ));
                widgets::technical_details(
                    ui,
                    ("dolphin_ini_technical_metadata", &file.sha256),
                    |ui| {
                        widgets::copyable_value(ui, "SHA-256", &file.sha256);
                        if file.duplicate_game_identity
                            || file.duplicate_filename
                            || file.duplicate_content
                        {
                            ui.label(format!(
                                "Duplicate identity: {} · filename: {} · content: {}",
                                file.duplicate_game_identity,
                                file.duplicate_filename,
                                file.duplicate_content
                            ));
                        }
                    },
                );
            });
        }
        if inventory.files.len() > MAX_RENDERED {
            ui.label(format!(
                "{} additional files omitted from this summary.",
                inventory.files.len() - MAX_RENDERED
            ));
        }
    });
    if !inventory.warnings.is_empty() {
        egui::CollapsingHeader::new(format!(
            "Inspection warnings ({})",
            inventory.warnings.len()
        ))
        .default_open(false)
        .show(ui, |ui| {
            for warning in inventory.warnings.iter().take(50) {
                ui.label(format!("{:?}: {}", warning.kind, warning.detail));
            }
            if inventory.warnings.len() > 50 {
                ui.label(format!(
                    "{} additional warnings omitted from this view.",
                    inventory.warnings.len() - 50
                ));
            }
        });
    }
}


pub(crate) fn show_dolphin_installation_unavailable(ui: &mut egui::Ui) {
    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(ui, "Preview and controlled installation", None);
    widgets::banner(
        ui,
        "Waiting for Dolphin profile",
        "Choose an eligible profile to resolve the exact GameSettings destination. Provider discovery does not require an existing GameSettings file.",
        widgets::StatusTone::Pending,
    );
}


pub(crate) fn dolphin_installation_label(kind: DolphinInstallationType) -> &'static str {
    match kind {
        DolphinInstallationType::Native => "Native",
        DolphinInstallationType::AppImage => "AppImage with explicit user directory",
        DolphinInstallationType::FlatpakUser => "Flatpak user",
        DolphinInstallationType::FlatpakSystem => "Flatpak system",
        DolphinInstallationType::Explicit => "Explicit user directory",
    }
}


pub(crate) fn dolphin_scope_label(scope: DolphinProfileScope) -> &'static str {
    match scope {
        DolphinProfileScope::User => "User profile",
        DolphinProfileScope::SystemInstallationUserProfile => "System install · user profile",
        DolphinProfileScope::Explicit => "Explicit scope",
    }
}


pub(crate) fn dolphin_directory_state_label(state: DolphinSettingsDirectoryState) -> &'static str {
    match state {
        DolphinSettingsDirectoryState::Available => "Exists",
        DolphinSettingsDirectoryState::Missing => "Missing",
        DolphinSettingsDirectoryState::UnsafePath => "Unsafe path",
        DolphinSettingsDirectoryState::NotDirectory => "Not a directory",
        DolphinSettingsDirectoryState::Unreadable => "Unreadable",
    }
}


pub(crate) fn dolphin_directory_state_tone(state: DolphinSettingsDirectoryState) -> widgets::StatusTone {
    match state {
        DolphinSettingsDirectoryState::Available => widgets::StatusTone::Success,
        DolphinSettingsDirectoryState::Missing => widgets::StatusTone::Pending,
        DolphinSettingsDirectoryState::UnsafePath => widgets::StatusTone::Blocked,
        DolphinSettingsDirectoryState::NotDirectory | DolphinSettingsDirectoryState::Unreadable => {
            widgets::StatusTone::Warning
        }
    }
}


pub(crate) fn dolphin_match_presentation(state: DolphinMatchState) -> (&'static str, widgets::StatusTone) {
    match state {
        DolphinMatchState::ExactGameIdMatch | DolphinMatchState::ExactGameIdAndRevisionMatch => (
            "Exact verified identity match",
            widgets::StatusTone::Success,
        ),
        DolphinMatchState::MultipleIniFilesForGame | DolphinMatchState::RevisionMismatch => {
            ("Ambiguous identity", widgets::StatusTone::Warning)
        }
        DolphinMatchState::NoVerifiedGameIdAvailable
        | DolphinMatchState::IdentityExtractionDeferred => {
            ("Verified Game ID unavailable", widgets::StatusTone::Pending)
        }
        DolphinMatchState::NoMatchingIniFound => {
            ("No matching Game INI", widgets::StatusTone::Pending)
        }
        DolphinMatchState::InvalidVerifiedGameId => {
            ("Invalid verified Game ID", widgets::StatusTone::Blocked)
        }
    }
}


pub(crate) fn show_pcsx2_profile_card(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    profile: &Pcsx2Profile,
    clipboard: &mut dyn ClipboardBackend,
) {
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            let profile_label = format!(
                "PCSX2 — {} ({})",
                pcsx2_installation_label(profile.installation_type),
                pcsx2_scope_label(profile.scope)
            );
            if profile.eligible {
                let selected = workflow.selected_pcsx2_profile_id.as_deref()
                    == Some(profile.profile_id.as_str());
                if ui.radio(selected, &profile_label).clicked() {
                    workflow.selected_pcsx2_profile_id = Some(profile.profile_id.clone());
                    workflow.pcsx2_inventory_profile_id = None;
                    workflow.pcsx2_inventory = CheatStepResource::NotLoaded;
                    workflow.pcsx2_activation = CheatActivationReadiness::Unknown;
                    workflow.pcsx2_activation_receiver = None;
                }
                widgets::status_badge(ui, "Ready for cheat setup", widgets::StatusTone::Success);
            } else {
                ui.add_enabled(false, egui::Button::selectable(false, profile_label));
                widgets::status_badge(ui, "Setup incomplete", widgets::StatusTone::Blocked);
            }
        });
        if !profile.blockers.is_empty() {
            ui.label("This PCSX2 setup cannot be used. Open Technical details to see what needs attention.");
        }
        widgets::technical_details(ui, ("pcsx2_profile_details", &profile.profile_id), |ui| {
            ui.label(format!("Profile ID: {}", profile.profile_id));
            if widgets::path_value(ui, "Configuration", &profile.configuration_path) {
                let _ = clipboard.set_text(profile.configuration_path.display().to_string());
            }
            for directory in &profile.patch_directories {
                ui.horizontal_wrapped(|ui| {
                    widgets::status_badge(
                        ui,
                        pcsx2_directory_state_label(directory.state),
                        pcsx2_directory_state_tone(directory.state),
                    );
                    ui.label(pcsx2_category_label(directory.category));
                    if widgets::path_value(ui, "Path", &directory.path) {
                        let _ = clipboard.set_text(directory.path.display().to_string());
                    }
                });
            }
            for blocker in &profile.blockers {
                ui.label(format!("{:?} — {}", blocker.kind, blocker.detail));
            }
        });
    });
}


pub(crate) fn show_pcsx2_inventory(
    ui: &mut egui::Ui,
    workflow: &CheatWorkflowState,
    inventory: &Pcsx2PnachInventory,
    clipboard: &mut dyn ClipboardBackend,
) {
    let category_count = |category| {
        inventory
            .files
            .iter()
            .filter(|file| file.category == category)
            .count()
    };
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            widgets::status_badge(
                ui,
                if inventory.complete {
                    "Complete"
                } else {
                    "Incomplete"
                },
                if inventory.complete {
                    widgets::StatusTone::Success
                } else {
                    widgets::StatusTone::Warning
                },
            );
            ui.strong(format!("{} PNACH files", inventory.files.len()));
            ui.label(format!("{} bytes inspected", inventory.bytes_inspected));
            ui.label(format!("{} entries visited", inventory.entries_visited));
        });
        ui.horizontal_wrapped(|ui| {
            for (category, label) in [
                (Pcsx2PatchCategory::Cheats, "Cheats"),
                (Pcsx2PatchCategory::WidescreenPatches, "Widescreen"),
                (Pcsx2PatchCategory::OtherPatches, "Other patches"),
                (Pcsx2PatchCategory::Unknown, "Unknown"),
            ] {
                widgets::status_badge(
                    ui,
                    format!("{label} · {}", category_count(category)),
                    widgets::StatusTone::Info,
                );
            }
        });
    });
    let match_result = match_pcsx2_inventory(
        inventory,
        ready_game_identity(workflow).and_then(GameIdentityReport::verified_pcsx2_crc),
        Some(&workflow.display_name),
    );
    let (match_label, match_tone) = pcsx2_match_presentation(match_result.state);
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            widgets::status_badge(ui, match_label, match_tone);
            ui.label(&match_result.reason);
        });
        ui.label("Exact matching requires a verified CRC calculated from the complete bounded boot ELF. Filename CRCs and comment titles remain candidates only.");
        for path in &match_result.matching_files {
            if widgets::path_value(ui, "Candidate", path) {
                let _ = clipboard.set_text(path.display().to_string());
            }
        }
    });
    egui::CollapsingHeader::new(format!("Inspected PNACH files ({})", inventory.files.len()))
        .default_open(false)
        .show(ui, |ui| {
            const MAX_RENDERED_PNACH_FILES: usize = 100;
            for file in inventory.files.iter().take(MAX_RENDERED_PNACH_FILES) {
                widgets::card(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(file.filename_stem.to_string_lossy());
                        widgets::status_badge(
                            ui,
                            pcsx2_category_label(file.category),
                            widgets::StatusTone::Info,
                        );
                        if let Some(crc) = &file.crc_candidate {
                            widgets::status_badge(ui, format!("CRC candidate · {crc}"), widgets::StatusTone::Pending);
                        }
                        ui.label(format!("{} patch entries", file.patch_entry_count));
                    });
                    if widgets::path_value(ui, "PNACH", &file.path) {
                        let _ = clipboard.set_text(file.path.display().to_string());
                    }
                    ui.label(format!(
                        "Enabled syntax {} · disabled syntax {} · unknown syntax {}",
                        file.enabled_patch_count,
                        file.disabled_patch_count,
                        file.unknown_patch_count
                    ));
                    if !file.title_candidates.is_empty() {
                        ui.label(format!("Comment title candidates: {}", file.title_candidates.join("; ")));
                    }
                    if !file.comments.is_empty() {
                        ui.label(format!(
                            "Retained comments: {}",
                            file.comments
                                .iter()
                                .take(3)
                                .cloned()
                                .collect::<Vec<_>>()
                                .join("; ")
                        ));
                    }
                    widgets::technical_details(ui, ("pcsx2_pnach_technical_metadata", &file.sha256), |ui| {
                        widgets::copyable_value(ui, "SHA-256", &file.sha256);
                        if file.duplicate_crc || file.duplicate_filename || file.duplicate_content {
                            ui.label(format!(
                                "Duplicate CRC: {} · filename: {} · content: {}",
                                file.duplicate_crc, file.duplicate_filename, file.duplicate_content
                            ));
                        }
                    });
                });
            }
            if inventory.files.len() > MAX_RENDERED_PNACH_FILES {
                ui.label(format!(
                    "{} additional files are retained in the bounded result but omitted from this summary.",
                    inventory.files.len() - MAX_RENDERED_PNACH_FILES
                ));
            }
        });
    if !inventory.warnings.is_empty() {
        egui::CollapsingHeader::new(format!(
            "Inspection warnings ({})",
            inventory.warnings.len()
        ))
        .default_open(false)
        .show(ui, |ui| {
            for warning in inventory.warnings.iter().take(50) {
                ui.label(format!("{:?}: {}", warning.kind, warning.detail));
            }
            if inventory.warnings.len() > 50 {
                ui.label(format!(
                    "{} additional warnings omitted from this view.",
                    inventory.warnings.len() - 50
                ));
            }
        });
    }
}


pub(crate) fn show_pcsx2_gamehacking(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    cheats_directory: Option<&Path>,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(
        ui,
        "GameHacking.org",
        Some(
            "Matches this local PS2 game against EmuWiz's private complete index cache; only the selected game's export is downloaded.",
        ),
    );
    let identity_ready = pcsx2_identity_for_workflow(workflow)
        .is_some_and(|identity| identity.verified_crc().is_some());
    let browser_import_open = workflow.browser_import.is_some();
    match &mut workflow.transaction {
        CheatTransactionState::Applying { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Installing selected cheats…");
            });
            return action;
        }
        CheatTransactionState::Result { result, .. } => {
            return show_beginner_install_result(ui, result);
        }
        CheatTransactionState::Idle | CheatTransactionState::Review { .. } => {}
    }
    match &mut workflow.pcsx2_gamehacking {
        CheatStepResource::NotLoaded => {
            widgets::status_badge(
                ui,
                if identity_ready {
                    "Ready to check"
                } else {
                    "Game identity incomplete"
                },
                if identity_ready {
                    widgets::StatusTone::Pending
                } else {
                    widgets::StatusTone::Blocked
                },
            );
            if widgets::action_button(
                ui,
                "Download",
                widgets::ActionStyle::Primary,
                identity_ready,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::FetchPcsx2GameHacking {
                    force_refresh: false,
                });
            }
        }
        CheatStepResource::Loading { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Checking GameHacking.org for this game…");
            });
        }
        CheatStepResource::Failed(message)
            if message.contains(GAMEHACKING_PROVIDER_CHALLENGE_MESSAGE) =>
        {
            action = show_browser_import_blocked_banner(
                ui,
                BrowserImportPlatform::PlayStation2,
                message,
                browser_import_open,
            );
        }
        CheatStepResource::Failed(message) => {
            widgets::banner(
                ui,
                "Could not check GameHacking.org",
                message,
                widgets::StatusTone::Warning,
            );
            if widgets::action_button(ui, "Try again", widgets::ActionStyle::Secondary, true)
                .clicked()
            {
                action = Some(CheatWorkflowAction::FetchPcsx2GameHacking {
                    force_refresh: false,
                });
            }
        }
        CheatStepResource::Ready(provider) => {
            if provider.cached_fallback {
                widgets::banner(
                    ui,
                    "Using cached GameHacking.org data",
                    "GameHacking.org blocked this automated request. Cached data is being used and may be stale. Try again later.",
                    widgets::StatusTone::Warning,
                );
            }
            let selectable_count = provider
                .candidates
                .iter()
                .filter(|candidate| candidate.selectable())
                .count();
            widgets::status_badge(
                ui,
                match provider.status {
                    GameHackingMatchStatus::Matched => "Game matched",
                    GameHackingMatchStatus::Candidates => "Confirm a candidate",
                    GameHackingMatchStatus::NoMatch => "No matching game",
                    GameHackingMatchStatus::IdentityConflict => "Game identity conflicted",
                    GameHackingMatchStatus::IdentityIncomplete => "Game identity incomplete",
                },
                if provider.status == GameHackingMatchStatus::Matched {
                    widgets::StatusTone::Success
                } else {
                    widgets::StatusTone::Warning
                },
            );
            ui.label(&provider.detail);
            for candidate in &provider.match_candidates {
                widgets::card(ui, |ui| {
                    ui.strong(&candidate.game.title);
                    ui.label(format!(
                        "Serial: {} · Region: {} · GameHacking game ID: {}",
                        candidate.game.serial.as_deref().unwrap_or("Unknown"),
                        candidate.game.region.as_deref().unwrap_or("Unknown"),
                        candidate.game.game_id
                    ));
                    ui.weak(format!("Match evidence: {}", candidate.strength.label()));
                    if widgets::action_button(
                        ui,
                        "Use this match",
                        widgets::ActionStyle::Primary,
                        true,
                    )
                    .clicked()
                    {
                        action = Some(CheatWorkflowAction::ConfirmPcsx2GameHackingMatch {
                            game_id: candidate.game.game_id,
                        });
                    }
                });
            }
            if let Some(game) = &provider.game {
                ui.weak(format!(
                    "{} compatible cheat{} · GameHacking game {}",
                    selectable_count,
                    if selectable_count == 1 { "" } else { "s" },
                    game.game_id
                ));
            }
            for candidate in provider
                .candidates
                .iter()
                .filter(|candidate| candidate.selectable())
            {
                widgets::card(ui, |ui| {
                    let mut selected = provider.selection.selected_ids.contains(&candidate.id);
                    if ui.checkbox(&mut selected, &candidate.name).changed() {
                        action = Some(CheatWorkflowAction::TogglePcsx2CheatSelected {
                            id: candidate.id.clone(),
                            selected,
                        });
                    }
                    if let Some(author) = &candidate.author {
                        ui.label(format!("Author: {author}"));
                    }
                    if let Some(description) = &candidate.description {
                        ui.label(format!("Notes: {description}"));
                    }
                });
            }
            ui.horizontal_wrapped(|ui| {
                if widgets::action_button(
                    ui,
                    "Refresh",
                    widgets::ActionStyle::Quiet,
                    identity_ready,
                )
                .clicked()
                {
                    action = Some(CheatWorkflowAction::FetchPcsx2GameHacking {
                        force_refresh: true,
                    });
                }
                if !browser_import_open
                    && widgets::action_button(
                        ui,
                        "Import through browser",
                        widgets::ActionStyle::Quiet,
                        identity_ready,
                    )
                    .clicked()
                {
                    action = Some(CheatWorkflowAction::OpenBrowserImport(
                        BrowserImportPlatform::PlayStation2,
                    ));
                }
                if matches!(workflow.transaction, CheatTransactionState::Idle)
                    && widgets::action_button(
                        ui,
                        "Install selected",
                        widgets::ActionStyle::Primary,
                        !provider.selection.selected_ids.is_empty()
                            && workflow.selected_pcsx2_profile_id.is_some()
                            && cheats_directory.is_some(),
                    )
                    .clicked()
                {
                    action = Some(CheatWorkflowAction::InstallSelectedPcsx2);
                }
            });
            if let (CheatTransactionState::Idle, CheatStepResource::Ready(response)) =
                (&workflow.transaction, &workflow.preview)
                && workflow.preview_request.as_ref() == Some(&response.key)
                && let CheatPreviewOutcome::Failed(failure) = &response.outcome
            {
                widgets::banner(
                    ui,
                    "Install failed",
                    &failure.to_string(),
                    widgets::StatusTone::Blocked,
                );
            }
        }
    }
    show_browser_import_open_error(ui, workflow);
    if let Some(state) = workflow.browser_import.as_mut()
        && state.plan.platform == BrowserImportPlatform::PlayStation2
        && let Some(import_action) = show_browser_import(ui, state)
    {
        action = Some(import_action);
    }
    if let CheatTransactionState::Review {
        plan,
        replacement_approved,
        ..
    } = &mut workflow.transaction
    {
        widgets::card(ui, |ui| {
            ui.strong("Install the selected cheats?");
            ui.label("EmuWiz will use the verified serial+CRC filename this PCSX2 build reads (falling back to CRC-only if no verified serial exists), keep a backup, and make this change undoable.");
            for entry in &plan.entries {
                ui.label(format!(
                    "Target file: {}/{}",
                    entry.destination_root.display, entry.destination_relative_path.display
                ));
            }
            if let CheatStepResource::Ready(response) = &workflow.preview
                && response
                    .pcsx2_generated
                    .as_ref()
                    .is_some_and(|generated| generated.legacy_migration_report.is_some())
            {
                ui.label(
                    "A legacy CRC-only file for this game was found and will be migrated into the file above, then stripped of EmuWiz content (kept, with a backup, as its own separately undoable step).",
                );
            }
            let replacement_required = plan.entries.iter().any(|entry| {
                entry.proposed_action
                    == archivefs_core::patch_manager::PreviewProposedAction::Replace
            });
            if replacement_required {
                ui.checkbox(
                    replacement_approved,
                    "I approve replacing the existing file shown in the preview",
                );
            }
            if widgets::action_button(
                ui,
                "Install",
                widgets::ActionStyle::Primary,
                !replacement_required || *replacement_approved,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::ConfirmApply);
            }
            if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true).clicked() {
                action = Some(CheatWorkflowAction::CancelApply);
            }
        });
    }
    action
}


/// The buttons offered when live GameHacking.org access is blocked. Kept
/// separate from the banner so the exact required wording lives in one
/// place and the same row can be reused by both platforms.
pub(crate) fn show_browser_import_blocked_banner(
    ui: &mut egui::Ui,
    platform: BrowserImportPlatform,
    provider_message: &str,
    dialog_open: bool,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    widgets::banner(
        ui,
        GAMEHACKING_BROWSER_IMPORT_BLOCKED_TITLE,
        GAMEHACKING_BROWSER_IMPORT_BLOCKED_BODY,
        widgets::StatusTone::Warning,
    );
    // The provider's own verbatim message (including the last-attempt
    // timestamp) stays visible underneath rather than being replaced.
    ui.weak(provider_message);
    if !dialog_open {
        ui.horizontal_wrapped(|ui| {
            if widgets::action_button(
                ui,
                "Import through browser",
                widgets::ActionStyle::Primary,
                true,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::OpenBrowserImport(platform));
            }
        });
    }
    action
}


/// Shows why the browser-assisted import panel could not be opened.
/// These are all local, actionable reasons - never an HTTP failure.
pub(crate) fn show_browser_import_open_error(ui: &mut egui::Ui, workflow: &CheatWorkflowState) {
    if let Some((headline, detail)) = &workflow.browser_import_open_error {
        ui.add_space(4.0);
        widgets::banner(ui, headline, detail, widgets::StatusTone::Blocked);
    }
}


/// The browser-assisted import panel. Shows every fact the person needs
/// before handing anything over - platform, local game, verified local
/// identity, GameHacking game ID, the exact URL expected, accepted
/// formats, the destination cache key, and whether an existing cached
/// response would be replaced - then the four import routes.
pub(crate) fn show_browser_import(
    ui: &mut egui::Ui,
    state: &mut BrowserImportState,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    ui.add_space(theme::SECTION_GAP);
    widgets::card(ui, |ui| {
        ui.label(
            egui::RichText::new("Import through browser")
                .size(17.0)
                .strong(),
        );
        ui.label(
            "EmuWiz never pretends to be a browser. Open the page yourself, then hand the saved page or its Text export back here.",
        );
        ui.add_space(6.0);
        ui.label(format!("Platform: {}", state.plan.platform_label));
        ui.label(format!(
            "Selected local game: {}",
            state.plan.local_game_title
        ));
        ui.label(format!(
            "Verified local identity: {}",
            state.plan.local_identity_summary
        ));
        ui.label(format!(
            "GameHacking game ID: {}",
            state.plan.gamehacking_game_id
        ));
        ui.label(format!(
            "Expected page URL: {}",
            state.plan.expected_source_url
        ));
        ui.label(format!(
            "Accepted formats: {}",
            state.plan.accepted_formats.join(" · ")
        ));
        for destination in &state.plan.destinations {
            match &destination.existing {
                Some(existing) => ui.label(format!(
                    "Destination ({}): {} — replaces an existing cached response ({}{})",
                    destination.kind.label(),
                    destination.cache_file_name,
                    existing.source,
                    existing
                        .retrieved_at_unix_seconds
                        .map(|value| format!(", Unix timestamp {value}"))
                        .unwrap_or_default()
                )),
                None => ui.label(format!(
                    "Destination ({}): {} — nothing cached yet",
                    destination.kind.label(),
                    destination.cache_file_name
                )),
            };
        }

        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            if widgets::action_button(
                ui,
                "Open game page in browser",
                widgets::ActionStyle::Primary,
                true,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::OpenGameHackingPageInBrowser);
            }
            if widgets::action_button(ui, "Copy page URL", widgets::ActionStyle::Quiet, true)
                .clicked()
            {
                action = Some(CheatWorkflowAction::CopyGameHackingPageUrl);
            }
        });
        ui.horizontal_wrapped(|ui| {
            if widgets::action_button(
                ui,
                "Import saved page",
                widgets::ActionStyle::Secondary,
                true,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::ImportBrowserSavedFile);
            }
            if widgets::action_button(
                ui,
                "Paste page/export",
                widgets::ActionStyle::Secondary,
                true,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::ToggleBrowserImportPaste(
                    !state.paste_open,
                ));
            }
            if widgets::action_button(
                ui,
                "Paste from clipboard",
                widgets::ActionStyle::Secondary,
                true,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::ImportBrowserClipboard);
            }
            if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true).clicked() {
                action = Some(CheatWorkflowAction::CloseBrowserImport);
            }
        });

        if state.plan.destinations.len() > 1 {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.weak("Treat the import as:");
                if ui
                    .selectable_label(state.kind.is_none(), "Detect from content")
                    .clicked()
                {
                    action = Some(CheatWorkflowAction::ChooseBrowserImportKind(None));
                }
                for destination in &state.plan.destinations {
                    if ui
                        .selectable_label(
                            state.kind == Some(destination.kind),
                            destination.kind.label(),
                        )
                        .clicked()
                    {
                        action = Some(CheatWorkflowAction::ChooseBrowserImportKind(Some(
                            destination.kind,
                        )));
                    }
                }
            });
        }

        if state.paste_open {
            ui.add_space(6.0);
            ui.label("Paste the complete page source, or the Text/PCSX2 export:");
            ui.add(
                egui::TextEdit::multiline(&mut state.pasted)
                    .desired_rows(6)
                    .desired_width(f32::INFINITY),
            );
            if widgets::action_button(
                ui,
                "Import pasted content",
                widgets::ActionStyle::Primary,
                !state.pasted.trim().is_empty(),
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::ImportBrowserPastedText);
            }
        }

        if let Some(notice) = &state.notice {
            ui.add_space(6.0);
            widgets::banner(ui, "Browser opened", notice, widgets::StatusTone::Info);
        }
        if let Some((headline, detail)) = &state.failure {
            ui.add_space(6.0);
            widgets::banner(ui, headline, detail, widgets::StatusTone::Blocked);
        }
        if let Some(outcome) = &state.outcome {
            ui.add_space(6.0);
            widgets::banner(
                ui,
                outcome.headline(),
                &format!(
                    "{} — GameHacking game {}",
                    outcome
                        .imported_title
                        .as_deref()
                        .unwrap_or(&state.plan.local_game_title),
                    outcome.gamehacking_game_id
                ),
                widgets::StatusTone::Success,
            );
            ui.label(format!(
                "{} cheat{} parsed · Action Replay {} · Gecko {} · Raw (format not declared) {}{}",
                outcome.cheat_count,
                if outcome.cheat_count == 1 { "" } else { "s" },
                outcome.action_replay_count,
                outcome.gecko_count,
                outcome.raw_unknown_count,
                if outcome.unsupported_count > 0 {
                    format!(" · Unsupported {}", outcome.unsupported_count)
                } else {
                    String::new()
                }
            ));
            ui.label(format!(
                "Cache destination: {}",
                outcome.cache_path.display()
            ));
            ui.weak(format!(
                "Provenance: {} · SHA-256 {}{}",
                outcome.provenance.source,
                outcome.provenance.stored_sha256,
                if outcome.replaced_existing_cache {
                    " · replaced the previous cached response"
                } else {
                    ""
                }
            ));
        }
    });
    action
}


/// GameCube-only GameHacking.org coverage: shows the matched title and
/// GameHacking game ID, the exact match evidence, and named cheats with
/// their author, notes, and identified code format. Only `ActionReplay`
/// and `Gecko` cheats are selectable and installable; `RawUnknown` and
/// `Unsupported` cheats are always shown checkbox-free, preview-only (see
/// `GameCubeCheatSelection::from_cheats`). No Wii yet.
/// Dolphin-family GameHacking.org coverage: shows the matched title and
/// GameHacking game ID, the exact match evidence, and named cheats with
/// their author, notes, and identified code format. Only `ActionReplay`
/// and `Gecko` cheats are selectable and installable; `RawUnknown` and
/// `Unsupported` cheats are always shown checkbox-free, preview-only (see
/// `GameCubeCheatSelection::from_cheats`).
pub(crate) fn show_gamecube_gamehacking(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    let is_wii = workflow.platform.as_deref() == Some("Wii");
    let platform = if is_wii { "Wii" } else { "GameCube" };
    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(
        ui,
        &format!("GameHacking.org ({platform})"),
        Some(if is_wii {
            "Matches the verified six-character Dolphin Game ID against EmuWiz's private Wii catalogue cache. Only explicitly labelled, safety-checked Action Replay and Gecko cheats can be installed into the resolved Dolphin GameSettings file."
        } else {
            "Matches this local GameCube game against EmuWiz's private complete index cache; only the selected game's cheats are downloaded. Only Action Replay and Gecko cheats can be installed into the real Dolphin GameSettings file."
        }),
    );
    let identity_ready = if is_wii {
        wii_identity_for_workflow(workflow)
            .is_some_and(|identity| identity.verified_game_id().is_some())
    } else {
        gamecube_identity_for_workflow(workflow)
            .is_some_and(|identity| identity.verified_game_id().is_some())
    };
    let browser_import_open = workflow.browser_import.is_some();
    if let Some(notice) = &workflow.transaction_notice {
        widgets::banner(
            ui,
            if notice.starts_with("Install failed") {
                "Installation failed"
            } else {
                "Installation cancelled"
            },
            notice,
            if notice.starts_with("Install failed") {
                widgets::StatusTone::Blocked
            } else {
                widgets::StatusTone::Info
            },
        );
    }
    if !bsfree_transaction_active(workflow) {
        match &workflow.transaction {
            CheatTransactionState::Applying { .. } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Installing selected cheats…");
                });
                return action;
            }
            CheatTransactionState::Result { result, .. } => {
                return show_beginner_install_result(ui, result);
            }
            CheatTransactionState::Idle | CheatTransactionState::Review { .. } => {}
        }
    }
    match &mut workflow.gamecube_gamehacking {
        CheatStepResource::NotLoaded => {
            widgets::status_badge(
                ui,
                if identity_ready {
                    "Ready to check"
                } else {
                    "Game identity incomplete"
                },
                if identity_ready {
                    widgets::StatusTone::Pending
                } else {
                    widgets::StatusTone::Blocked
                },
            );
            if widgets::action_button(
                ui,
                "Check GameHacking.org",
                widgets::ActionStyle::Primary,
                identity_ready,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::FetchGameCubeGameHacking {
                    force_refresh: false,
                });
            }
        }
        CheatStepResource::Loading { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Checking GameHacking.org for this game…");
            });
        }
        CheatStepResource::Failed(message) if workflow.gamecube_gamehacking_blocked && !is_wii => {
            // A confirmed Cloudflare/anti-bot block, not a generic failure:
            // retrying immediately cannot help (core-side cooldown gating
            // already prevents hammering the origin again), so no Retry is
            // offered here at all - matches
            // `GAMEHACKING_PROVIDER_CHALLENGE_MESSAGE` exactly, which is
            // also why `message` itself is rendered verbatim rather than a
            // separately-maintained copy of the wording.
            action = show_browser_import_blocked_banner(
                ui,
                BrowserImportPlatform::GameCube,
                message,
                browser_import_open,
            );
        }
        CheatStepResource::Failed(message) if workflow.gamecube_gamehacking_blocked => {
            widgets::banner(
                ui,
                "GameHacking.org access blocked",
                message,
                widgets::StatusTone::Warning,
            );
            ui.label(
                "Offline option: save the Wii game page in your browser, then import it with the existing Wii page importer. EmuWiz validates the platform and Game ID before caching it.",
            );
        }
        CheatStepResource::Failed(message) => {
            widgets::banner(
                ui,
                "Could not check GameHacking.org",
                message,
                widgets::StatusTone::Warning,
            );
            if widgets::action_button(ui, "Try again", widgets::ActionStyle::Secondary, true)
                .clicked()
            {
                action = Some(CheatWorkflowAction::FetchGameCubeGameHacking {
                    force_refresh: false,
                });
            }
        }
        CheatStepResource::Ready(state) => {
            if state.cached_fallback {
                widgets::banner(
                    ui,
                    "Using cached GameHacking.org data",
                    "GameHacking.org blocked this automated request. Cached data is being used and may be stale. Try again later.",
                    widgets::StatusTone::Warning,
                );
            }
            widgets::status_badge(
                ui,
                match state.status {
                    GameHackingGameCubeMatchStatus::Matched => "Game matched",
                    GameHackingGameCubeMatchStatus::Candidates => "Confirm a candidate",
                    GameHackingGameCubeMatchStatus::NoMatch => "No matching game",
                    GameHackingGameCubeMatchStatus::IdentityConflict => "Game identity conflicted",
                    GameHackingGameCubeMatchStatus::IdentityIncomplete => {
                        "Game identity incomplete"
                    }
                },
                if state.status == GameHackingGameCubeMatchStatus::Matched {
                    widgets::StatusTone::Success
                } else {
                    widgets::StatusTone::Warning
                },
            );
            ui.label(&state.detail);
            for candidate in &state.match_candidates {
                widgets::card(ui, |ui| {
                    ui.strong(&candidate.game.title);
                    ui.label(format!(
                        "Game ID: {} · Region: {} · GameHacking game ID: {}",
                        candidate
                            .game
                            .dolphin_game_id
                            .as_deref()
                            .unwrap_or("Unknown"),
                        candidate.game.region.as_deref().unwrap_or("Unknown"),
                        candidate.game.game_id
                    ));
                    ui.weak(format!("Match evidence: {}", candidate.strength.label()));
                    if widgets::action_button(
                        ui,
                        "Use this match",
                        widgets::ActionStyle::Primary,
                        true,
                    )
                    .clicked()
                    {
                        action = Some(CheatWorkflowAction::ConfirmGameCubeGameHackingMatch {
                            game_id: candidate.game.game_id,
                        });
                    }
                });
            }
            if let Some(game) = &state.game {
                ui.strong(&game.title);
                ui.label(format!(
                    "Game ID: {} · Region: {} · GameHacking game ID: {}",
                    game.dolphin_game_id.as_deref().unwrap_or("Unknown"),
                    game.region.as_deref().unwrap_or("Unknown"),
                    game.game_id
                ));
                ui.weak(format!(
                    "{} cheat{} · GameHacking game {}",
                    state.cheats.len(),
                    if state.cheats.len() == 1 { "" } else { "s" },
                    game.game_id
                ));
            }
            for (position, cheat) in state.cheats.iter().enumerate() {
                let entry = state
                    .selection
                    .entries
                    .iter()
                    .find(|entry| entry.index == position)
                    .cloned();
                widgets::card(ui, |ui| {
                    if let Some(entry) = &entry
                        && entry.selectable
                    {
                        let mut selected = entry.selected;
                        if ui.checkbox(&mut selected, &cheat.name).changed() {
                            action = Some(
                                CheatWorkflowAction::ToggleGameCubeGameHackingCheatSelected {
                                    index: entry.index,
                                    selected,
                                },
                            );
                        }
                        if entry.already_managed {
                            ui.weak("Already installed by EmuWiz.");
                        }
                    } else {
                        ui.strong(&cheat.name);
                    }
                    if let Some(author) = &cheat.author {
                        ui.label(format!("Author: {author}"));
                    }
                    if let Some(description) = &cheat.description {
                        ui.label(format!("Notes: {description}"));
                    }
                    ui.weak(format!(
                        "Code format: {}",
                        match cheat.code_format {
                            GameCubeCodeFormat::ActionReplay => "Action Replay",
                            GameCubeCodeFormat::Gecko => "Gecko",
                            GameCubeCodeFormat::RawUnknown => "Raw (format not declared)",
                            GameCubeCodeFormat::Unsupported => "Unsupported",
                        }
                    ));
                    if entry.is_none_or(|entry| !entry.selectable) {
                        ui.weak(
                            "Preview only - EmuWiz never installs a cheat whose Action Replay/Gecko format wasn't explicitly labelled by GameHacking.org.",
                        );
                    }
                });
            }
            ui.horizontal_wrapped(|ui| {
                if widgets::action_button(
                    ui,
                    "Refresh",
                    widgets::ActionStyle::Quiet,
                    identity_ready,
                )
                .clicked()
                {
                    action = Some(CheatWorkflowAction::FetchGameCubeGameHacking {
                        force_refresh: true,
                    });
                }
                if !is_wii
                    && !browser_import_open
                    && widgets::action_button(
                        ui,
                        "Import through browser",
                        widgets::ActionStyle::Quiet,
                        identity_ready,
                    )
                    .clicked()
                {
                    action = Some(CheatWorkflowAction::OpenBrowserImport(
                        BrowserImportPlatform::GameCube,
                    ));
                }
                if state.game.is_some()
                    && matches!(workflow.transaction, CheatTransactionState::Idle)
                    && widgets::action_button(
                        ui,
                        "Install selected",
                        widgets::ActionStyle::Primary,
                        state.selection.can_apply()
                            && workflow.selected_dolphin_profile_id.is_some(),
                    )
                    .clicked()
                {
                    action = Some(CheatWorkflowAction::InstallSelectedGameCubeGameHacking);
                }
                let removable_count = state
                    .selection
                    .entries
                    .iter()
                    .filter(|entry| entry.selected && entry.already_managed)
                    .count();
                if state.game.is_some()
                    && matches!(workflow.transaction, CheatTransactionState::Idle)
                    && widgets::action_button(
                        ui,
                        "Remove selected",
                        widgets::ActionStyle::Secondary,
                        removable_count > 0 && workflow.selected_dolphin_profile_id.is_some(),
                    )
                    .clicked()
                {
                    action = Some(CheatWorkflowAction::RemoveSelectedGameCubeGameHacking);
                }
            });
            if let (CheatTransactionState::Idle, CheatStepResource::Ready(response)) =
                (&workflow.transaction, &workflow.preview)
                && workflow.preview_request.as_ref() == Some(&response.key)
                && let CheatPreviewOutcome::Failed(failure) = &response.outcome
            {
                widgets::banner(
                    ui,
                    "Install failed",
                    &failure.to_string(),
                    widgets::StatusTone::Blocked,
                );
            }
        }
    }
    show_browser_import_open_error(ui, workflow);
    if let Some(state) = workflow.browser_import.as_mut()
        && state.plan.platform == BrowserImportPlatform::GameCube
        && let Some(import_action) = show_browser_import(ui, state)
    {
        action = Some(import_action);
    }
    let skipped_raw_unknown: Vec<String> = match &workflow.gamecube_gamehacking {
        CheatStepResource::Ready(state) => state
            .cheats
            .iter()
            .filter(|cheat| {
                matches!(
                    cheat.code_format,
                    GameCubeCodeFormat::RawUnknown | GameCubeCodeFormat::Unsupported
                )
            })
            .map(|cheat| cheat.name.clone())
            .collect(),
        _ => Vec::new(),
    };
    let gamecube_gamehacking_affected: Vec<StagedGameCubeCheat> = match &workflow.preview {
        CheatStepResource::Ready(response) => response
            .gamecube_gamehacking_generated
            .as_ref()
            .map(|generated| generated.staged.affected.clone())
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let gamecube_install_context = match &workflow.preview {
        CheatStepResource::Ready(response) => response
            .gamecube_gamehacking_generated
            .as_ref()
            .map(|generated| (generated.profile.clone(), generated.staged.path.clone())),
        _ => None,
    };
    let review_is_gamecube_gamehacking = !gamecube_gamehacking_affected.is_empty()
        || matches!(
            &workflow.preview,
            CheatStepResource::Ready(response) if response.gamecube_gamehacking_generated.is_some()
        );
    if review_is_gamecube_gamehacking
        && let CheatTransactionState::Review {
            plan,
            replacement_approved,
            ..
        } = &mut workflow.transaction
    {
        widgets::card(ui, |ui| {
            ui.strong("Install or remove the selected cheats?");
            ui.label(
                "EmuWiz will write only to the [Gecko]/[ActionReplay] sections of this exact Dolphin GameSettings file, keep a backup, and make this change undoable.",
            );
            if let Some((profile, staging_path)) = &gamecube_install_context {
                ui.label(format!(
                    "Selected Dolphin executable: {}",
                    profile
                        .resolved
                        .emulator_executable
                        .as_deref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "native/system fallback".to_string())
                ));
                ui.label(format!(
                    "Discovery evidence: {}",
                    profile.resolved.discovery_evidence.join("; ")
                ));
                ui.label(format!(
                    "Dolphin user root: {}",
                    profile.resolved.data_user_root.display()
                ));
                ui.label(format!(
                    "GameSettings directory: {}",
                    profile.game_settings_path.display()
                ));
                ui.label(format!(
                    "Staging artifact (not destination): {}",
                    staging_path.display()
                ));
            }
            for entry in &plan.entries {
                ui.label(format!(
                    "Target file: {}/{}",
                    entry.destination_root.display, entry.destination_relative_path.display
                ));
            }
            for cheat in &gamecube_gamehacking_affected {
                ui.label(format!(
                    "{} → [{}]",
                    cheat.name,
                    match cheat.code_format {
                        GameCubeCodeFormat::ActionReplay => "ActionReplay",
                        GameCubeCodeFormat::Gecko => "Gecko",
                        GameCubeCodeFormat::RawUnknown | GameCubeCodeFormat::Unsupported => "n/a",
                    }
                ));
            }
            if !skipped_raw_unknown.is_empty() {
                ui.label(format!(
                    "Skipped (preview-only, format not declared by GameHacking.org): {}",
                    skipped_raw_unknown.join(", ")
                ));
            }
            let replacement_required = plan.entries.iter().any(|entry| {
                entry.proposed_action
                    == archivefs_core::patch_manager::PreviewProposedAction::Replace
            });
            if replacement_required {
                ui.checkbox(
                    replacement_approved,
                    "I approve replacing the existing file shown in the preview",
                );
            }
            if widgets::action_button(
                ui,
                "Confirm",
                widgets::ActionStyle::Primary,
                !replacement_required || *replacement_approved,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::ConfirmApply);
            }
            if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true).clicked() {
                action = Some(CheatWorkflowAction::CancelApply);
            }
        });
    }
    action
}


/// The BSFree Archive GameCube section inside Cheats & Mods: search the
/// optional local SQLite database by the selected archive's title, confirm a
/// candidate when several match, select supported cheats, and install through
/// the same shared preview/apply/rollback pipeline as every other Dolphin
/// cheat source. Unsupported and browse-only records stay visible but can
/// never enter the apply batch.
pub(crate) fn show_bsfree_gamecube(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(
        ui,
        "BSFree Archive",
        Some(
            "An optional historical cheat database. GameCube cheats can be installed with Dolphin. \
             Other BSFree formats remain browse only.",
        ),
    );

    if bsfree_transaction_active(workflow) {
        match &workflow.transaction {
            CheatTransactionState::Applying { .. } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Installing selected cheats…");
                });
                return action;
            }
            CheatTransactionState::Result { result, .. } => {
                return show_bsfree_install_result(ui, workflow, result);
            }
            CheatTransactionState::Idle | CheatTransactionState::Review { .. } => {}
        }
    }

    let identity_ready = gamecube_identity_for_workflow(workflow)
        .is_some_and(|identity| identity.verified_game_id().is_some());
    let profile_selected = workflow.selected_dolphin_profile_id.is_some();

    match &mut workflow.bsfree_gamecube {
        CheatStepResource::NotLoaded => {
            if identity_ready {
                // Auto-search once as soon as the archive's verified Game ID is
                // available, so matching BSFree cheats appear alongside the
                // other cheat sources without any CLI usage.
                let search_title = workflow.display_name.clone();
                action = Some(CheatWorkflowAction::FetchBsFreeGameCube { search_title });
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Searching BSFree Archive for this game…");
                });
            } else {
                widgets::status_badge(ui, "Game identity incomplete", widgets::StatusTone::Blocked);
                ui.label("EmuWiz needs a verified Dolphin Game ID before it can search BSFree.");
            }
        }
        CheatStepResource::Loading { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading the local BSFree database…");
            });
        }
        CheatStepResource::Failed(message) => {
            widgets::banner(
                ui,
                "BSFree Archive unavailable",
                message,
                widgets::StatusTone::Warning,
            );
            if message.contains("not installed")
                || message.contains("not installed, enabled and validated")
            {
                ui.label(
                    "Download or import the optional historical database from the Cheat Sources \
                     page, then search again.",
                );
            }
            if identity_ready
                && widgets::action_button(ui, "Try again", widgets::ActionStyle::Secondary, true)
                    .clicked()
            {
                let search_title = workflow.display_name.clone();
                action = Some(CheatWorkflowAction::FetchBsFreeGameCube { search_title });
            }
        }
        CheatStepResource::Ready(state) => {
            ui.horizontal_wrapped(|ui| {
                ui.label("Search title:");
                ui.add(egui::TextEdit::singleline(&mut state.search_title).desired_width(280.0));
                if ui
                    .add_enabled(identity_ready, egui::Button::new("Search BSFree"))
                    .clicked()
                {
                    action = Some(CheatWorkflowAction::FetchBsFreeGameCube {
                        search_title: state.search_title.trim().to_string(),
                    });
                }
            });
            ui.weak(&state.detail);

            match state.status {
                BsFreeGameCubeSearchStatus::Candidates => {
                    for candidate in &state.candidates {
                        widgets::card(ui, |ui| {
                            ui.strong(&candidate.matched_bsfree_title);
                            ui.label(format!(
                                "Version: {} · BSFree game UID {}",
                                candidate
                                    .matched_bsfree_version
                                    .as_deref()
                                    .unwrap_or("not supplied"),
                                candidate.matched_bsfree_game_upstream_uid
                            ));
                            ui.weak(&candidate.region_evidence);
                            if widgets::action_button(
                                ui,
                                "Use this game",
                                widgets::ActionStyle::Primary,
                                true,
                            )
                            .clicked()
                            {
                                action = Some(CheatWorkflowAction::ConfirmBsFreeGameCubeMatch {
                                    upstream_uid: candidate.matched_bsfree_game_upstream_uid,
                                });
                            }
                        });
                    }
                }
                BsFreeGameCubeSearchStatus::NoMatch => {}
                BsFreeGameCubeSearchStatus::Matched => {
                    if let Some(game) = &state.game {
                        widgets::card(ui, |ui| {
                            ui.strong(&game.matched_bsfree_title);
                            ui.label(format!(
                                "Matched for: {} · Game ID {}",
                                game.archive_title, game.archive_game_id
                            ));
                            ui.weak(&game.region_evidence);
                            ui.weak(
                                "Matched by platform and title. BSFree carries no verified game \
                                 revision, so review the cheats before applying.",
                            );
                        });
                    }
                    for (position, cheat) in state.cheats.iter().enumerate() {
                        let entry = state
                            .selection
                            .entries
                            .iter()
                            .find(|entry| entry.index == position)
                            .cloned();
                        let (status_label, tone) = bsfree_cheat_status(cheat, &state.analysis);
                        widgets::card(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if let Some(entry) = &entry
                                    && entry.selectable
                                {
                                    let mut selected = entry.selected;
                                    if ui.checkbox(&mut selected, &cheat.name).changed() {
                                        action = Some(
                                            CheatWorkflowAction::ToggleBsFreeGameCubeCheatSelected {
                                                index: entry.index,
                                                selected,
                                            },
                                        );
                                    }
                                } else {
                                    ui.strong(&cheat.name);
                                }
                                widgets::status_badge(ui, status_label, tone);
                            });
                            if let Some(author) = &cheat.author {
                                ui.label(format!("Author: {author}"));
                            }
                            if let Some(note) = &cheat.note {
                                ui.label(format!("Notes: {note}"));
                            }
                            if entry.is_none_or(|entry| !entry.selectable) {
                                ui.weak(bsfree_browse_only_reason(cheat.code_format));
                            }
                            ui.collapsing("Code", |ui| {
                                for line in &cheat.code_lines {
                                    ui.monospace(line);
                                }
                            });
                        });
                    }
                    let selected_count = state.selection.selected_count();
                    ui.horizontal_wrapped(|ui| {
                        if widgets::action_button(
                            ui,
                            "Select all supported",
                            widgets::ActionStyle::Quiet,
                            state.selection.selectable_count() > 0,
                        )
                        .clicked()
                        {
                            action = Some(CheatWorkflowAction::SelectAllBsFreeGameCubeCheats);
                        }
                        if widgets::action_button(
                            ui,
                            "Clear selection",
                            widgets::ActionStyle::Quiet,
                            selected_count > 0,
                        )
                        .clicked()
                        {
                            action = Some(CheatWorkflowAction::ClearAllBsFreeGameCubeCheats);
                        }
                        if matches!(workflow.transaction, CheatTransactionState::Idle)
                            && widgets::action_button(
                                ui,
                                format!("Install {selected_count} cheats"),
                                widgets::ActionStyle::Primary,
                                state.selection.can_apply()
                                    && profile_selected
                                    && matches!(workflow.transaction, CheatTransactionState::Idle),
                            )
                            .clicked()
                        {
                            action = Some(CheatWorkflowAction::InstallSelectedBsFreeGameCube);
                        }
                    });
                    if !profile_selected {
                        ui.weak("Choose an eligible Dolphin profile to enable installation.");
                    }
                }
            }
        }
    }

    // The shared review card for a BSFree preview, outside the mutable borrow
    // of `bsfree_gamecube`.
    if matches!(workflow.transaction, CheatTransactionState::Review { .. }) {
        action = show_bsfree_review_card(ui, workflow).or(action);
    }
    action
}


/// Per-cheat status for the BSFree Wii list, using the destination-based
/// analysis computed when the search completed. The finding vocabulary is the
/// shared generalized one, so the states ("Ready", "Already installed",
/// "Conflict", "Browse only") are identical to the GameCube list.
pub(crate) fn bsfree_wii_cheat_status(
    cheat: &BsFreeWiiCheat,
    analysis: &[BsFreeWiiDedupFinding],
) -> (&'static str, widgets::StatusTone) {
    if !cheat.code_format.is_installable() {
        return ("Browse only", widgets::StatusTone::Pending);
    }
    let findings = analysis
        .iter()
        .filter(|finding| finding.cheat_upstream_id == cheat.upstream_id)
        .collect::<Vec<_>>();
    if findings.iter().any(|finding| {
        matches!(
            finding.kind,
            BsFreeDedupFindingKind::AlreadyInstalled
                | BsFreeDedupFindingKind::AlreadyInstalledDifferentName
        )
    }) {
        return ("Already installed", widgets::StatusTone::Info);
    }
    if findings
        .iter()
        .any(|finding| finding.kind.blocks_selection())
    {
        return ("Conflict", widgets::StatusTone::Blocked);
    }
    ("Ready", widgets::StatusTone::Success)
}


/// Concise browse-only reason for an unsupported/malformed BSFree Wii code.
pub(crate) fn bsfree_wii_browse_only_reason(code_format: BsFreeWiiCodeFormat) -> &'static str {
    match code_format {
        BsFreeWiiCodeFormat::GeckoEquivalent | BsFreeWiiCodeFormat::ActionReplayNative => {
            "Supported by Dolphin."
        }
        BsFreeWiiCodeFormat::Unsupported => {
            "Browse only — this code contains an Action Replay command Dolphin refuses to run."
        }
        BsFreeWiiCodeFormat::Malformed => {
            "Browse only — this code is encrypted, malformed, or from an unverified Wii device."
        }
    }
}


/// The BSFree Wii section in Cheats & Mods. Mirrors `show_bsfree_gamecube`;
/// only the verified hex-pair subset is selectable, and identity is the
/// archive's verified Dolphin Wii Game ID.
pub(crate) fn show_bsfree_wii(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(
        ui,
        "BSFree Archive",
        Some(
            "An optional historical cheat database. Only verified Wii hex-pair \
             codes can be installed with Dolphin; encrypted and unverified formats \
             remain browse only.",
        ),
    );

    if bsfree_transaction_active(workflow) {
        match &workflow.transaction {
            CheatTransactionState::Applying { .. } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Installing selected cheats…");
                });
                return action;
            }
            CheatTransactionState::Result { result, .. } => {
                return show_bsfree_install_result(ui, workflow, result);
            }
            CheatTransactionState::Idle | CheatTransactionState::Review { .. } => {}
        }
    }

    let identity_ready = wii_identity_for_workflow(workflow)
        .is_some_and(|identity| identity.verified_game_id().is_some());
    let profile_selected = workflow.selected_dolphin_profile_id.is_some();

    match &mut workflow.bsfree_wii {
        CheatStepResource::NotLoaded => {
            if identity_ready {
                let search_title = workflow.display_name.clone();
                action = Some(CheatWorkflowAction::FetchBsFreeWii { search_title });
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Searching BSFree Archive for this game…");
                });
            } else {
                widgets::status_badge(ui, "Game identity incomplete", widgets::StatusTone::Blocked);
                ui.label(
                    "EmuWiz needs a verified Dolphin Wii Game ID before it can search BSFree.",
                );
            }
        }
        CheatStepResource::Loading { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading the local BSFree database…");
            });
        }
        CheatStepResource::Failed(message) => {
            widgets::banner(
                ui,
                "BSFree Archive unavailable",
                message,
                widgets::StatusTone::Warning,
            );
            if message.contains("not installed")
                || message.contains("not installed, enabled and validated")
            {
                ui.label(
                    "Download or import the optional historical database from the Cheat Sources \
                     page, then search again.",
                );
            }
            if identity_ready
                && widgets::action_button(ui, "Try again", widgets::ActionStyle::Secondary, true)
                    .clicked()
            {
                let search_title = workflow.display_name.clone();
                action = Some(CheatWorkflowAction::FetchBsFreeWii { search_title });
            }
        }
        CheatStepResource::Ready(state) => {
            ui.horizontal_wrapped(|ui| {
                ui.label("Search title:");
                ui.add(egui::TextEdit::singleline(&mut state.search_title).desired_width(280.0));
                if ui
                    .add_enabled(identity_ready, egui::Button::new("Search BSFree"))
                    .clicked()
                {
                    action = Some(CheatWorkflowAction::FetchBsFreeWii {
                        search_title: state.search_title.trim().to_string(),
                    });
                }
            });
            ui.weak(&state.detail);

            match state.status {
                BsFreeWiiSearchStatus::Candidates => {
                    for candidate in &state.candidates {
                        widgets::card(ui, |ui| {
                            ui.strong(&candidate.matched_bsfree_title);
                            ui.label(format!(
                                "Version: {} · BSFree game UID {}",
                                candidate
                                    .matched_bsfree_version
                                    .as_deref()
                                    .unwrap_or("not supplied"),
                                candidate.matched_bsfree_game_upstream_uid
                            ));
                            ui.weak(&candidate.region_evidence);
                            if widgets::action_button(
                                ui,
                                "Use this game",
                                widgets::ActionStyle::Primary,
                                true,
                            )
                            .clicked()
                            {
                                action = Some(CheatWorkflowAction::ConfirmBsFreeWiiMatch {
                                    upstream_uid: candidate.matched_bsfree_game_upstream_uid,
                                });
                            }
                        });
                    }
                }
                BsFreeWiiSearchStatus::NoMatch => {
                    ui.weak(
                        "No BSFree Wii records match. The shipped BSFree catalogue contains no \
                         Wii rows; GameHacking.org remains the Wii cheat source.",
                    );
                }
                BsFreeWiiSearchStatus::Matched => {
                    if let Some(game) = &state.game {
                        widgets::card(ui, |ui| {
                            ui.strong(&game.matched_bsfree_title);
                            ui.label(format!(
                                "Matched for: {} · Game ID {}",
                                game.archive_title, game.archive_game_id
                            ));
                            ui.weak(&game.region_evidence);
                            ui.weak(
                                "Matched by platform and title. BSFree carries no verified game \
                                 revision, so review the cheats before applying.",
                            );
                        });
                    }
                    for (position, cheat) in state.cheats.iter().enumerate() {
                        let entry = state
                            .selection
                            .entries
                            .iter()
                            .find(|entry| entry.index == position)
                            .cloned();
                        let (status_label, tone) = bsfree_wii_cheat_status(cheat, &state.analysis);
                        widgets::card(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if let Some(entry) = &entry
                                    && entry.selectable
                                {
                                    let mut selected = entry.selected;
                                    if ui.checkbox(&mut selected, &cheat.name).changed() {
                                        action = Some(
                                            CheatWorkflowAction::ToggleBsFreeWiiCheatSelected {
                                                index: entry.index,
                                                selected,
                                            },
                                        );
                                    }
                                } else {
                                    ui.strong(&cheat.name);
                                }
                                widgets::status_badge(ui, status_label, tone);
                            });
                            if let Some(author) = &cheat.author {
                                ui.label(format!("Author: {author}"));
                            }
                            if let Some(note) = &cheat.note {
                                ui.label(format!("Notes: {note}"));
                            }
                            if entry.is_none_or(|entry| !entry.selectable) {
                                ui.weak(bsfree_wii_browse_only_reason(cheat.code_format));
                            }
                            ui.collapsing("Code", |ui| {
                                for line in &cheat.code_lines {
                                    ui.monospace(line);
                                }
                            });
                        });
                    }
                    let selected_count = state.selection.selected_count();
                    ui.horizontal_wrapped(|ui| {
                        if widgets::action_button(
                            ui,
                            "Select all supported",
                            widgets::ActionStyle::Quiet,
                            state.selection.selectable_count() > 0,
                        )
                        .clicked()
                        {
                            action = Some(CheatWorkflowAction::SelectAllBsFreeWiiCheats);
                        }
                        if widgets::action_button(
                            ui,
                            "Clear selection",
                            widgets::ActionStyle::Quiet,
                            selected_count > 0,
                        )
                        .clicked()
                        {
                            action = Some(CheatWorkflowAction::ClearAllBsFreeWiiCheats);
                        }
                        if matches!(workflow.transaction, CheatTransactionState::Idle)
                            && widgets::action_button(
                                ui,
                                format!("Install {selected_count} cheats"),
                                widgets::ActionStyle::Primary,
                                state.selection.can_apply()
                                    && profile_selected
                                    && matches!(workflow.transaction, CheatTransactionState::Idle),
                            )
                            .clicked()
                        {
                            action = Some(CheatWorkflowAction::InstallSelectedBsFreeWii);
                        }
                    });
                    if !profile_selected {
                        ui.weak("Choose an eligible Dolphin profile to enable installation.");
                    }
                }
            }
        }
    }

    if matches!(workflow.transaction, CheatTransactionState::Review { .. }) {
        action = show_bsfree_review_card(ui, workflow).or(action);
    }
    action
}


/// Per-cheat status for the BSFree list, using the destination-based analysis
/// computed when the search completed. Never exposes raw converter terminology
/// by default.
pub(crate) fn bsfree_cheat_status(
    cheat: &BsFreeGameCubeCheat,
    analysis: &[BsFreeDedupFinding],
) -> (&'static str, widgets::StatusTone) {
    if !cheat.code_format.is_installable() {
        return ("Browse only", widgets::StatusTone::Pending);
    }
    let findings = analysis
        .iter()
        .filter(|finding| finding.cheat_upstream_id == cheat.upstream_id)
        .collect::<Vec<_>>();
    if findings.iter().any(|finding| {
        matches!(
            finding.kind,
            BsFreeDedupFindingKind::AlreadyInstalled
                | BsFreeDedupFindingKind::AlreadyInstalledDifferentName
        )
    }) {
        return ("Already installed", widgets::StatusTone::Info);
    }
    if findings
        .iter()
        .any(|finding| finding.kind.blocks_selection())
    {
        return ("Conflict", widgets::StatusTone::Blocked);
    }
    ("Ready", widgets::StatusTone::Success)
}


/// Concise browse-only reason for an unsupported/malformed BSFree code.
pub(crate) fn bsfree_browse_only_reason(code_format: BsFreeGameCubeCodeFormat) -> &'static str {
    match code_format {
        BsFreeGameCubeCodeFormat::GeckoEquivalent
        | BsFreeGameCubeCodeFormat::ActionReplayNative => "Supported by Dolphin.",
        BsFreeGameCubeCodeFormat::Unsupported => {
            "Browse only — this code contains an Action Replay command Dolphin refuses to run."
        }
        BsFreeGameCubeCodeFormat::Malformed => {
            "Browse only — this code format cannot be installed yet."
        }
    }
}


/// The BSFree review card: selected game, Dolphin profile, selected cheat
/// count, files that would change, already-installed items, conflicts, and
/// the unsupported entries that are excluded. Nothing here mutates anything.
pub(crate) fn show_bsfree_review_card(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    let CheatStepResource::Ready(response) = &workflow.preview else {
        return action;
    };
    // Both the GameCube and Wii BSFree flows render through this card; the
    // staged artifact, profile, findings and skipped lists have the same
    // shape (both write the same Dolphin GameSettings structure), so the card
    // binds whichever source produced the preview.
    let matched_title_gc = match &workflow.bsfree_gamecube {
        CheatStepResource::Ready(state) => state
            .game
            .as_ref()
            .map(|game| game.matched_bsfree_title.clone()),
        _ => None,
    };
    let matched_title_wii = match &workflow.bsfree_wii {
        CheatStepResource::Ready(state) => state
            .game
            .as_ref()
            .map(|game| game.matched_bsfree_title.clone()),
        _ => None,
    };
    let (staged, profile, findings, skipped_duplicates, skipped_unselectable, matched_title) =
        match &response.bsfree_gamecube_generated {
            Some(generated) => (
                &generated.staged,
                &generated.profile,
                &generated.findings,
                &generated.skipped_duplicates,
                &generated.skipped_unselectable,
                matched_title_gc,
            ),
            None => match &response.bsfree_wii_generated {
                Some(generated) => (
                    &generated.staged,
                    &generated.profile,
                    &generated.findings,
                    &generated.skipped_duplicates,
                    &generated.skipped_unselectable,
                    matched_title_wii,
                ),
                None => return action,
            },
        };
    let CheatTransactionState::Review {
        plan,
        replacement_approved,
        ..
    } = &mut workflow.transaction
    else {
        return action;
    };
    widgets::card(ui, |ui| {
        let count = staged.affected.len();
        ui.strong(format!(
            "Apply {count} cheat{s}?",
            s = if count == 1 { "" } else { "s" }
        ));
        ui.label(
            "EmuWiz will write only to the [Gecko]/[ActionReplay] sections of this exact \
             Dolphin GameSettings file, keep a backup, and make this change undoable.",
        );
        if let Some(title) = &matched_title {
            ui.label(format!("BSFree game: {title}"));
        }
        ui.label(format!(
            "Selected Dolphin executable: {}",
            profile
                .resolved
                .emulator_executable
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "native/system fallback".to_string())
        ));
        ui.label(format!(
            "GameSettings directory: {}",
            profile.game_settings_path.display()
        ));
        ui.label(format!(
            "Staging artifact (not destination): {}",
            staged.path.display()
        ));
        for entry in &plan.entries {
            ui.label(format!(
                "Target file: {}/{}",
                entry.destination_root.display, entry.destination_relative_path.display
            ));
        }
        for cheat in &staged.affected {
            ui.label(format!(
                "{} → [{}]",
                cheat.name,
                match cheat.code_format {
                    GameCubeCodeFormat::ActionReplay => "ActionReplay",
                    GameCubeCodeFormat::Gecko => "Gecko",
                    GameCubeCodeFormat::RawUnknown | GameCubeCodeFormat::Unsupported => "n/a",
                }
            ));
        }
        let installed: Vec<&DolphinDedupFinding> = findings
            .iter()
            .filter(|finding| {
                matches!(
                    finding.kind,
                    BsFreeDedupFindingKind::AlreadyInstalled
                        | BsFreeDedupFindingKind::AlreadyInstalledDifferentName
                )
            })
            .collect();
        if !installed.is_empty() {
            ui.label(format!(
                "Already installed (not applied again): {}",
                installed
                    .iter()
                    .map(|finding| finding.cheat_name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let conflicts: Vec<&DolphinDedupFinding> = findings
            .iter()
            .filter(|finding| finding.kind.blocks_selection())
            .collect();
        if !conflicts.is_empty() {
            ui.label(format!(
                "Conflicts (blocked, not applied): {}",
                conflicts
                    .iter()
                    .map(|finding| finding.cheat_name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !skipped_unselectable.is_empty() {
            ui.label(format!(
                "Unsupported (excluded): {}",
                skipped_unselectable.join(", ")
            ));
        }
        if !skipped_duplicates.is_empty() {
            ui.label(format!(
                "Skipped duplicates: {}",
                skipped_duplicates.join(", ")
            ));
        }
        let replacement_required = plan.entries.iter().any(|entry| {
            entry.proposed_action == archivefs_core::patch_manager::PreviewProposedAction::Replace
        });
        if replacement_required {
            ui.checkbox(
                replacement_approved,
                "I approve replacing the existing file shown in the preview",
            );
        }
        if widgets::action_button(
            ui,
            "Confirm",
            widgets::ActionStyle::Primary,
            !replacement_required || *replacement_approved,
        )
        .clicked()
        {
            action = Some(CheatWorkflowAction::ConfirmApply);
        }
        if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true).clicked() {
            action = Some(CheatWorkflowAction::CancelApply);
        }
    });
    action
}


/// The BSFree install result card: how many cheats were added, the
/// already-installed/skipped/conflict/unsupported details from the provider's
/// analysis, and Undo (rollback) through the same shared history flow every
/// other install uses.
pub(crate) fn show_bsfree_install_result(
    ui: &mut egui::Ui,
    workflow: &CheatWorkflowState,
    result: &SharedApplyResult,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    widgets::card(ui, |ui| {
        let (count, skipped, conflicts, unsupported) = match &workflow.preview {
            CheatStepResource::Ready(response) => response
                .bsfree_gamecube_generated
                .as_ref()
                .map(|generated| {
                    (
                        generated.staged.affected.len(),
                        generated.skipped_duplicates.clone(),
                        generated
                            .findings
                            .iter()
                            .filter(|finding| finding.kind.blocks_selection())
                            .map(|finding| finding.cheat_name.clone())
                            .collect::<Vec<_>>(),
                        generated.skipped_unselectable.clone(),
                    )
                })
                .or_else(|| {
                    response.bsfree_wii_generated.as_ref().map(|generated| {
                        (
                            generated.staged.affected.len(),
                            generated.skipped_duplicates.clone(),
                            generated
                                .findings
                                .iter()
                                .filter(|finding| finding.kind.blocks_selection())
                                .map(|finding| finding.cheat_name.clone())
                                .collect::<Vec<_>>(),
                            generated.skipped_unselectable.clone(),
                        )
                    })
                })
                .unwrap_or_default(),
            _ => Default::default(),
        };
        match result.journal.status {
            SharedApplyStatus::Success => {
                widgets::status_badge(
                    ui,
                    format!(
                        "{count} cheat{s} added",
                        s = if count == 1 { "" } else { "s" }
                    ),
                    widgets::StatusTone::Success,
                );
            }
            SharedApplyStatus::PartialFailure => {
                widgets::status_badge(
                    ui,
                    "Installed with some problems",
                    widgets::StatusTone::Warning,
                );
            }
            SharedApplyStatus::Failed => {
                widgets::status_badge(ui, "Install failed", widgets::StatusTone::Blocked);
            }
            SharedApplyStatus::DryRun => {
                widgets::status_badge(ui, "Dry run complete", widgets::StatusTone::Info);
            }
        }
        if !skipped.is_empty() {
            ui.label(format!(
                "Already installed or skipped: {}",
                skipped.join(", ")
            ));
        }
        if !conflicts.is_empty() {
            ui.label(format!("Conflicts (not applied): {}", conflicts.join(", ")));
        }
        if !unsupported.is_empty() {
            ui.label(format!(
                "Unsupported (browse only): {}",
                unsupported.join(", ")
            ));
        }
        for entry in &result.journal.entries {
            ui.label(format!(
                "Live target: {}/{}",
                entry.plan_entry.destination_root.display,
                entry.plan_entry.destination_relative_path.display
            ));
            for failure in &entry.failures {
                ui.label(format!(
                    "Failed stage: {:?} · target: {} · {}",
                    failure.kind,
                    failure
                        .path
                        .as_ref()
                        .map(|path| path.display.as_str())
                        .unwrap_or("unknown"),
                    failure.detail
                ));
            }
        }
        let rollback_available = result.journal_path.is_some()
            && matches!(
                result.journal.status,
                SharedApplyStatus::Success | SharedApplyStatus::PartialFailure
            );
        if widgets::action_button(
            ui,
            "Undo installation",
            widgets::ActionStyle::Destructive,
            rollback_available,
        )
        .clicked()
        {
            action = Some(CheatWorkflowAction::RollbackInstall);
        }
    });
    action
}


pub(crate) fn pcsx2_installation_label(kind: Pcsx2InstallationType) -> &'static str {
    match kind {
        Pcsx2InstallationType::Native => "Native",
        Pcsx2InstallationType::NativeAlternate => "Native (alternate data location)",
        Pcsx2InstallationType::FlatpakUser => "Flatpak user",
        Pcsx2InstallationType::FlatpakSystem => "Flatpak system",
        Pcsx2InstallationType::Portable => "Portable / AppImage / explicit configuration",
    }
}


pub(crate) fn pcsx2_scope_label(scope: Pcsx2ProfileScope) -> &'static str {
    match scope {
        Pcsx2ProfileScope::User => "User profile",
        Pcsx2ProfileScope::SystemInstallationUserProfile => "System install · user profile",
        Pcsx2ProfileScope::Portable => "Portable scope",
    }
}


pub(crate) fn pcsx2_category_label(category: Pcsx2PatchCategory) -> &'static str {
    match category {
        Pcsx2PatchCategory::Cheats => "Cheats",
        Pcsx2PatchCategory::WidescreenPatches => "Widescreen patches",
        Pcsx2PatchCategory::OtherPatches => "Other PNACH patches",
        Pcsx2PatchCategory::Unknown => "Unknown PNACH category",
    }
}


pub(crate) fn pcsx2_directory_state_label(state: Pcsx2PatchDirectoryState) -> &'static str {
    match state {
        Pcsx2PatchDirectoryState::Available => "Exists",
        Pcsx2PatchDirectoryState::Missing => "Missing",
        Pcsx2PatchDirectoryState::UnsafePath => "Unsafe path",
        Pcsx2PatchDirectoryState::NotDirectory => "Not a directory",
        Pcsx2PatchDirectoryState::Unreadable => "Unreadable",
    }
}


pub(crate) fn pcsx2_directory_state_tone(state: Pcsx2PatchDirectoryState) -> widgets::StatusTone {
    match state {
        Pcsx2PatchDirectoryState::Available => widgets::StatusTone::Success,
        Pcsx2PatchDirectoryState::Missing => widgets::StatusTone::Pending,
        Pcsx2PatchDirectoryState::UnsafePath => widgets::StatusTone::Blocked,
        Pcsx2PatchDirectoryState::NotDirectory | Pcsx2PatchDirectoryState::Unreadable => {
            widgets::StatusTone::Warning
        }
    }
}


pub(crate) fn pcsx2_match_presentation(state: Pcsx2MatchState) -> (&'static str, widgets::StatusTone) {
    match state {
        Pcsx2MatchState::ExactCrcMatch => ("Exact CRC match", widgets::StatusTone::Success),
        Pcsx2MatchState::MultiplePnachFilesForSameCrc => {
            ("Ambiguous CRC match", widgets::StatusTone::Warning)
        }
        Pcsx2MatchState::CandidateByFilenameOrTitleOnly => {
            ("Unverified title candidate", widgets::StatusTone::Warning)
        }
        Pcsx2MatchState::NoVerifiedGameCrcAvailable => (
            "Verified game CRC unavailable",
            widgets::StatusTone::Pending,
        ),
        Pcsx2MatchState::NoMatchingPnachFound => {
            ("No matching PNACH", widgets::StatusTone::Pending)
        }
        Pcsx2MatchState::InvalidVerifiedGameCrc => {
            ("Invalid verified CRC", widgets::StatusTone::Blocked)
        }
        Pcsx2MatchState::IdentityExtractionDeferred => (
            "Exact game version could not be confirmed",
            widgets::StatusTone::Pending,
        ),
    }
}


pub(crate) fn ready_game_identity(workflow: &CheatWorkflowState) -> Option<&GameIdentityReport> {
    match &workflow.identity {
        CheatStepResource::Ready((request, report))
            if request.archive_path == workflow.archive_path
                && request.platform == workflow.platform
                && request.adapter == workflow.adapter
                && report.archive_path == workflow.archive_path =>
        {
            Some(report)
        }
        _ => None,
    }
}


pub(crate) fn pcsx2_identity_for_workflow(workflow: &CheatWorkflowState) -> Option<Pcsx2GameIdentity> {
    let report = ready_game_identity(workflow)?;
    let mut identity = Pcsx2GameIdentity::from_report(workflow.display_name.clone(), report);
    if workflow.region.is_some() {
        identity.region = workflow.region.clone();
    }
    Some(identity)
}


pub(crate) fn gamecube_identity_for_workflow(workflow: &CheatWorkflowState) -> Option<GameCubeGameIdentity> {
    let report = ready_game_identity(workflow)?;
    Some(GameCubeGameIdentity::from_report(
        workflow.display_name.clone(),
        report,
    ))
}


/// Opens the installed, validated BSFree Archive SQLite catalogue (the pinned
/// SHA-256 re-validation makes this too heavy for the UI thread, so it only
/// ever runs on a background thread).
pub(crate) fn open_installed_bsfree_catalogue() -> Result<BsFreeCatalogue, String> {
    let paths = BsFreePaths::at(default_bsfree_source_root().map_err(|error| error.to_string())?);
    BsFreeCatalogue::open_installed(&paths).map_err(|error| error.to_string())
}


/// Builds the GUI state from a BSFree search/confirm outcome.
pub(crate) fn bsfree_gui_state_from_outcome(
    outcome: BsFreeGameCubeSearchOutcome,
    search_title: String,
) -> BsFreeGameCubeGuiState {
    let cheats = outcome.cheats;
    let selection = BsFreeGameCubeCheatSelection::from_cheats(&cheats, &parse_dolphin_ini(""));
    BsFreeGameCubeGuiState {
        status: outcome.status,
        detail: outcome.detail,
        candidates: outcome.candidates,
        game: outcome.game,
        cheats,
        selection,
        analysis: Vec::new(),
        search_title,
    }
}


pub(crate) fn bsfree_wii_gui_state_from_outcome(
    outcome: BsFreeWiiSearchOutcome,
    search_title: String,
) -> BsFreeWiiGuiState {
    let cheats = outcome.cheats;
    let selection = BsFreeWiiCheatSelection::from_cheats(&cheats, &parse_dolphin_ini(""));
    BsFreeWiiGuiState {
        status: outcome.status,
        detail: outcome.detail,
        candidates: outcome.candidates,
        game: outcome.game,
        cheats,
        selection,
        analysis: Vec::new(),
        search_title,
    }
}


pub(crate) fn bsfree_wii_gui_state_from_matched(
    upstream_uid: i64,
    archive_title: &str,
    cheats: Vec<BsFreeWiiCheat>,
) -> BsFreeWiiGuiState {
    let selection = BsFreeWiiCheatSelection::from_cheats(&cheats, &parse_dolphin_ini(""));
    BsFreeWiiGuiState {
        status: BsFreeWiiSearchStatus::Matched,
        detail: "Matched BSFree Wii game; review the cheats before applying.".to_string(),
        candidates: Vec::new(),
        game: Some(BsFreeWiiMatch {
            archive_title: archive_title.to_string(),
            archive_game_id: String::new(),
            matched_bsfree_game_upstream_uid: upstream_uid,
            matched_bsfree_title: archive_title.to_string(),
            matched_bsfree_version: None,
            region_evidence: String::new(),
            requires_review: true,
            detail: "confirmed BSFree Wii match".to_string(),
        }),
        cheats,
        selection,
        analysis: Vec::new(),
        search_title: archive_title.to_string(),
    }
}


pub(crate) fn wii_identity_for_workflow(workflow: &CheatWorkflowState) -> Option<WiiGameIdentity> {
    let report = ready_game_identity(workflow)?;
    Some(WiiGameIdentity::from_report(
        workflow.display_name.clone(),
        report,
    ))
}


/// Whether the current shared transaction/preview belongs to the BSFree
/// GameCube or Wii flow rather than the GameHacking.org flow. The sources
/// share one `workflow.transaction`; only the source whose preview artifact
/// is present renders the applying/result/review states.
pub(crate) fn bsfree_transaction_active(workflow: &CheatWorkflowState) -> bool {
    matches!(
        &workflow.preview,
        CheatStepResource::Ready(response)
            if response.bsfree_gamecube_generated.is_some()
                || response.bsfree_wii_generated.is_some()
    )
}


pub(crate) fn cheat_preview_key(workflow: &CheatWorkflowState) -> CheatPreviewRequestKey {
    let profile_id = match workflow.adapter {
        CheatEmulatorAdapter::RetroArch => workflow.selected_profile_id.clone(),
        CheatEmulatorAdapter::Pcsx2 => workflow.selected_pcsx2_profile_id.clone(),
        CheatEmulatorAdapter::Dolphin => workflow.selected_dolphin_profile_id.clone(),
        CheatEmulatorAdapter::Xenia => workflow.selected_xenia_profile_id.clone(),
        CheatEmulatorAdapter::Unsupported => None,
    };
    CheatPreviewRequestKey {
        archive_path: workflow.archive_path.clone(),
        platform: workflow.platform.clone(),
        adapter: workflow.adapter,
        profile_id,
        source_mode: workflow.source_mode,
        source_id: workflow.selected_source_id.clone(),
        snapshot_id: match &workflow.source_fetch {
            CheatStepResource::Ready(result) => Some(result.manifest.archive_sha256.clone()),
            _ => None,
        },
    }
}


pub(crate) fn preview_identity(
    workflow: &CheatWorkflowState,
    kind: PreviewIdentityKind,
    value: Option<&str>,
    revision: Option<u16>,
) -> PreviewIdentity {
    PreviewIdentity {
        kind,
        state: if value.is_some() {
            PreviewIdentityState::Verified
        } else {
            PreviewIdentityState::Missing
        },
        value: value.map(str::to_owned),
        archive_path: workflow.archive_path.clone(),
        revision,
    }
}


/// The selected profile's own resolved cheat directory, or `None` when no
/// eligible profile is selected or its path cannot be represented exactly.
/// EmuWiz never invents a default cheat directory.
pub(crate) fn selected_retroarch_cheat_root(
    workflow: &CheatWorkflowState,
    profiles: &RetroArchProfilesState,
) -> Option<PathBuf> {
    let selected = workflow.selected_profile_id.as_deref()?;
    let RetroArchProfilesState::Ready(discovery) = profiles else {
        return None;
    };
    let profile = discovery
        .profiles
        .iter()
        .find(|profile| profile.eligible && profile.profile_id == selected)?;
    let root = profile.cheat_destination_root.as_ref()?;
    (!root.lossy).then(|| PathBuf::from(&root.display))
}


/// The content file's basename without extension - the strongest filename
/// identity available, and the name RetroArch itself shows for the content.
pub(crate) fn cheat_content_basename(workflow: &CheatWorkflowState) -> Option<String> {
    workflow
        .archive_path
        .file_stem()
        .and_then(|value| value.to_str())
        .map(str::to_string)
}

/// Everything stage 4 needs: the request key it is bound to, the verified
/// catalogue root to match against, and the archive identity to match.
/// `None` whenever any of those is not yet available, which is what keeps
/// matching from running against a half-built context.
/// Why matching cannot start right now. Every variant carries an exact,
/// user-facing reason - this is what makes clicking "Find matching cheat
/// files" with an unmet prerequisite a visible, explained "blocked" state
/// instead of the click silently doing nothing (see `start_cheat_candidate_match`).
/// Marks a `CheatStepResource::Failed` message for stage 4 as a blocked
/// prerequisite rather than an actual worker failure, so the UI can show a
/// visibly different state for the two ("blocked with an exact reason" vs
/// "failed with an exact error" - both required, and distinguishable).
pub(crate) const CHEAT_MATCH_BLOCKED_PREFIX: &str = "\u{1}blocked\u{1}";


/// Gathers everything stage 4 needs, or explains exactly why it cannot
/// start yet. Assumes the caller has already confirmed the adapter and
/// source mode are RetroArch + EmuWiz trusted catalogue - the only
/// context "Find matching cheat files" is ever shown in.
pub(crate) fn build_cheat_candidate_request(
    workflow: &CheatWorkflowState,
    profiles: &RetroArchProfilesState,
) -> Result<(CheatPreviewRequestKey, PathBuf, CheatCandidateArchive), CheatCandidatePrerequisite> {
    if selected_retroarch_cheat_root(workflow, profiles).is_none() {
        return Err(CheatCandidatePrerequisite::ProfileCheatDirectoryUnresolved);
    }
    let CheatStepResource::Ready(fetch) = &workflow.source_fetch else {
        return Err(CheatCandidatePrerequisite::CatalogueNotRetrieved);
    };
    if fetch.local_catalogue_path.lossy {
        return Err(CheatCandidatePrerequisite::CatalogueLocalPathUnavailable);
    }
    let identity = ready_game_identity(workflow);
    Ok((
        cheat_preview_key(workflow),
        PathBuf::from(&fetch.local_catalogue_path.display),
        CheatCandidateArchive {
            display_name: workflow.display_name.clone(),
            platform: workflow.platform.clone(),
            region: workflow.region.clone(),
            serial: identity
                .and_then(|report| report.verified_value(IdentityKind::Ps2Serial))
                .map(str::to_owned),
            content_hash: identity
                .and_then(GameIdentityReport::verified_loose_rom_sha256)
                .map(str::to_owned),
            content_basename: cheat_content_basename(workflow),
        },
    ))
}


/// Binds the currently selected RetroArch game into the identity and
/// destination the local-cheat-file install action needs - the same
/// archive path, platform, region, and verified evidence
/// `build_cheat_candidate_request` already binds for the trusted-catalogue
/// journey, reused unchanged rather than re-derived.
pub(crate) fn local_cheat_install_context(
    workflow: &CheatWorkflowState,
    profiles: &RetroArchProfilesState,
) -> user_cheat_import_page::LocalCheatInstallContext {
    let identity = ready_game_identity(workflow);
    let mut evidence = vec![CheatJourneyIdentityEvidence {
        kind: CheatJourneyIdentityEvidenceKind::CanonicalLibraryRecord,
        value: workflow.archive_path.display().to_string(),
    }];
    if let Some(serial) = identity.and_then(|report| report.verified_value(IdentityKind::Ps2Serial))
    {
        evidence.push(CheatJourneyIdentityEvidence {
            kind: CheatJourneyIdentityEvidenceKind::ProductCode,
            value: serial.to_string(),
        });
    }
    if let Some(hash) = identity.and_then(GameIdentityReport::verified_loose_rom_sha256) {
        evidence.push(CheatJourneyIdentityEvidence {
            kind: CheatJourneyIdentityEvidenceKind::ContentHash,
            value: hash.to_string(),
        });
    }
    let game = CheatJourneyGameIdentity {
        state: CheatJourneyIdentityState::Verified,
        selected_archive: workflow.archive_path.clone(),
        identity_key: workflow.archive_path.display().to_string(),
        archive: CheatCandidateArchive {
            display_name: workflow.display_name.clone(),
            platform: workflow.platform.clone(),
            region: workflow.region.clone(),
            serial: identity
                .and_then(|report| report.verified_value(IdentityKind::Ps2Serial))
                .map(str::to_owned),
            content_hash: identity
                .and_then(GameIdentityReport::verified_loose_rom_sha256)
                .map(str::to_owned),
            content_basename: cheat_content_basename(workflow),
        },
        evidence,
    };
    let destination =
        selected_retroarch_cheat_root(workflow, profiles).map(|root| CheatDestinationRequest {
            profile_cheat_root: root,
            platform: workflow.platform.clone(),
            content_basename: cheat_content_basename(workflow),
            playlist_name: None,
            catalogue_name: workflow.display_name.clone(),
        });
    user_cheat_import_page::LocalCheatInstallContext { game, destination }
}


/// Binds the currently selected PCSX2 game into the identity and profile
/// the local-`.pnach`-file install action needs, the same way
/// `local_cheat_install_context` binds RetroArch's. `profile` is `None`
/// when no eligible PCSX2 profile is selected yet - the install action
/// stays disabled with that exact reason rather than guessing one.
pub(crate) fn local_pcsx2_install_context(
    workflow: &CheatWorkflowState,
    profiles: &Pcsx2ProfilesState,
) -> Option<user_cheat_import_page::LocalPcsx2InstallContext> {
    let identity = pcsx2_identity_for_workflow(workflow)?;
    let profile = workflow
        .selected_pcsx2_profile_id
        .as_deref()
        .and_then(|selected| {
            let Pcsx2ProfilesState::Ready(discovery) = profiles else {
                return None;
            };
            discovery
                .profiles
                .iter()
                .find(|profile| profile.eligible && profile.profile_id == selected)
                .cloned()
        });
    Some(user_cheat_import_page::LocalPcsx2InstallContext { identity, profile })
}


/// Binds the currently selected Dolphin game into the already-resolved
/// candidate and profile the local-`.ini`-file install action needs -
/// exactly the same verified game ID/revision resolution
/// `dolphin_gamehacking_request_key` already uses (network-free: no
/// provider fetch required), pointed at the selected profile's own
/// configuration root. `None` when no verified Dolphin identity or no
/// eligible profile is selected yet; the install action stays disabled
/// with that exact reason rather than guessing either one.
pub(crate) fn local_dolphin_install_context(
    workflow: &CheatWorkflowState,
    profiles: &DolphinProfilesState,
) -> Option<user_cheat_import_page::LocalDolphinInstallContext> {
    let report = ready_game_identity(workflow)?;
    let game_id = report.verified_dolphin_game_id()?.to_string();
    let is_wii = workflow.platform.as_deref() == Some("Wii");
    let revision = if is_wii {
        wii_identity_for_workflow(workflow)?.candidate_revision
    } else {
        report.verified_dolphin_revision()
    };
    let profile_id = workflow.selected_dolphin_profile_id.clone()?;
    let DolphinProfilesState::Ready(discovery) = profiles else {
        return None;
    };
    let profile = discovery
        .profiles
        .iter()
        .find(|profile| profile.eligible && profile.profile_id == profile_id)?;
    let configuration_path = profile.configuration_path.clone();
    let candidate = DolphinCandidate {
        game_id: game_id.clone(),
        region: None,
        revision,
        path: configuration_path
            .join("GameSettings")
            .join(format!("{game_id}.ini")),
        cheat_count: 0,
        enabled_count: 0,
        evidence: Vec::new(),
        installable: true,
    };
    Some(user_cheat_import_page::LocalDolphinInstallContext {
        candidate,
        platform: if is_wii {
            archivefs_core::patch_manager::CheatPlatform::Wii
        } else {
            archivefs_core::patch_manager::CheatPlatform::GameCube
        },
        configuration_path,
        profile_id,
    })
}


/// Binds the local Xenia picker to the selected game's already-resolved XEX
/// Title ID and the explicitly selected eligible Xenia profile. No provider
/// lookup is performed: this context is entirely local and read-only.
pub(crate) fn local_xenia_install_context(
    workflow: &CheatWorkflowState,
    profiles: &XeniaProfilesState,
) -> user_cheat_import_page::LocalXeniaInstallContext {
    let title_id = ready_game_identity(workflow)
        .and_then(GameIdentityReport::verified_xex_title_id)
        .map(str::to_string);
    let profile = workflow
        .selected_xenia_profile_id
        .as_deref()
        .and_then(|selected| {
            let XeniaProfilesState::Ready(discovery) = profiles else {
                return None;
            };
            discovery
                .profiles
                .iter()
                .find(|profile| profile.eligible && profile.profile_id == selected)
                .cloned()
        });
    user_cheat_import_page::LocalXeniaInstallContext { title_id, profile }
}


/// The private directory generated cheat files are staged into before they
/// enter the transaction pipeline. Kept beside the other managed roots so
/// it is never a directory the user browses or an emulator reads.
pub(crate) fn default_generated_cheat_staging_root() -> Result<PathBuf, String> {
    default_shared_backup_root()
        .map(|root| {
            root.parent()
                .map(|parent| parent.join("generated-cheats"))
                .unwrap_or_else(|| root.join("generated-cheats"))
        })
        .map_err(|error| format!("Staging root unavailable: {}", error.detail))
}


/// The private directory staged Dolphin GameSettings files are written
/// into before they enter the transaction pipeline - the Dolphin
/// equivalent of `default_generated_cheat_staging_root`.
/// The private directory a local PCSX2 `.pnach` install stages its merged
/// output into before it enters the transaction pipeline - kept separate
/// from every other adapter's staging root for the same reason as
/// `default_generated_dolphin_staging_root`.
pub(crate) fn default_generated_pcsx2_local_staging_root() -> Result<PathBuf, String> {
    default_shared_backup_root()
        .map(|root| {
            root.parent()
                .map(|parent| parent.join("generated-pcsx2-local"))
                .unwrap_or_else(|| root.join("generated-pcsx2-local"))
        })
        .map_err(|error| format!("Staging root unavailable: {}", error.detail))
}


/// The private directory a local Dolphin `.ini` install stages its merged
/// GameSettings output into before it enters the transaction pipeline -
/// kept separate from the provider-driven `generated-dolphin` staging
/// root for the same reason `generated-pcsx2-local` is kept separate from
/// PCSX2's own provider staging root.
pub(crate) fn default_generated_dolphin_local_staging_root() -> Result<PathBuf, String> {
    default_shared_backup_root()
        .map(|root| {
            root.parent()
                .map(|parent| parent.join("generated-dolphin-local"))
                .unwrap_or_else(|| root.join("generated-dolphin-local"))
        })
        .map_err(|error| format!("Staging root unavailable: {}", error.detail))
}


pub(crate) fn default_generated_dolphin_staging_root() -> Result<PathBuf, String> {
    default_shared_backup_root()
        .map(|root| {
            root.parent()
                .map(|parent| parent.join("generated-dolphin"))
                .unwrap_or_else(|| root.join("generated-dolphin"))
        })
        .map_err(|error| format!("Staging root unavailable: {}", error.detail))
}


/// The private directory staged GameHacking.org GameCube installs are
/// written into - kept separate from `generated-dolphin` (the bundled
/// Dolphin Gecko catalogue's own staging root) so the two install
/// sources' journal-visible staging paths are never confused.
pub(crate) fn default_generated_gamecube_gamehacking_staging_root() -> Result<PathBuf, String> {
    default_shared_backup_root()
        .map(|root| {
            root.parent()
                .map(|parent| parent.join("generated-gamecube-gamehacking"))
                .unwrap_or_else(|| root.join("generated-gamecube-gamehacking"))
        })
        .map_err(|error| format!("Staging root unavailable: {}", error.detail))
}


pub(crate) fn default_generated_dolphin_gamehacking_staging_root(is_wii: bool) -> Result<PathBuf, String> {
    if !is_wii {
        return default_generated_gamecube_gamehacking_staging_root();
    }
    default_shared_backup_root()
        .map(|root| {
            root.parent()
                .map(|parent| parent.join("generated-wii-gamehacking"))
                .unwrap_or_else(|| root.join("generated-wii-gamehacking"))
        })
        .map_err(|error| format!("Staging root unavailable: {}", error.detail))
}


pub(crate) fn default_generated_xenia_staging_root() -> Result<PathBuf, String> {
    default_shared_backup_root()
        .map(|root| {
            root.parent()
                .map(|parent| parent.join("generated-xenia"))
                .unwrap_or_else(|| root.join("generated-xenia"))
        })
        .map_err(|error| format!("Staging root unavailable: {}", error.detail))
}


pub(crate) fn build_cheat_preview_request(
    workflow: &CheatWorkflowState,
    retroarch_profiles: &RetroArchProfilesState,
    pcsx2_profiles: &Pcsx2ProfilesState,
    dolphin_profiles: &DolphinProfilesState,
) -> Option<(CheatPreviewRequestKey, CheatPreviewWork)> {
    let key = cheat_preview_key(workflow);
    let identity = ready_game_identity(workflow);
    match workflow.adapter {
        CheatEmulatorAdapter::Pcsx2 => {
            let selected = workflow.selected_pcsx2_profile_id.as_deref()?;
            let Pcsx2ProfilesState::Ready(discovery) = pcsx2_profiles else {
                return None;
            };
            let profile = discovery
                .profiles
                .iter()
                .find(|profile| profile.eligible && profile.profile_id == selected)?;
            let CheatStepResource::Ready(inventory) = &workflow.pcsx2_inventory else {
                return None;
            };
            let verified_crc = identity.and_then(GameIdentityReport::verified_pcsx2_crc);
            let matched =
                match_pcsx2_inventory(inventory, verified_crc, Some(&workflow.display_name));
            let strength = match matched.state {
                Pcsx2MatchState::ExactCrcMatch | Pcsx2MatchState::MultiplePnachFilesForSameCrc => {
                    PreviewMatchStrength::VerifiedExact
                }
                Pcsx2MatchState::CandidateByFilenameOrTitleOnly => PreviewMatchStrength::Candidate,
                Pcsx2MatchState::InvalidVerifiedGameCrc => PreviewMatchStrength::Ambiguous,
                _ => PreviewMatchStrength::Unsupported,
            };
            let source_items = matched
                .matching_files
                .iter()
                .filter_map(|path| {
                    let file = inventory.files.iter().find(|file| file.path == *path)?;
                    let relative = path.strip_prefix(&profile.configuration_path).ok()?;
                    Some(PreviewSourceItem {
                        adapter: PreviewAdapter::Pcsx2,
                        source_path: path.clone(),
                        expected_source_digest: Some(file.sha256.clone()),
                        destination_relative_paths: vec![relative.to_path_buf()],
                        match_strength: strength,
                    })
                })
                .collect();
            Some((
                key,
                CheatPreviewWork::Shared(SharedPreviewRequest {
                    adapter: PreviewAdapter::Pcsx2,
                    selected_archive: workflow.archive_path.clone(),
                    platform: workflow.platform.clone(),
                    identity: preview_identity(
                        workflow,
                        PreviewIdentityKind::Pcsx2ExecutableCrc,
                        verified_crc,
                        None,
                    ),
                    destination_root: profile.configuration_path.clone(),
                    source_items,
                }),
            ))
        }
        CheatEmulatorAdapter::Dolphin => {
            let selected = workflow.selected_dolphin_profile_id.as_deref()?;
            let DolphinProfilesState::Ready(discovery) = dolphin_profiles else {
                return None;
            };
            let profile = discovery
                .profiles
                .iter()
                .find(|profile| profile.eligible && profile.profile_id == selected)?;
            let CheatStepResource::Ready(inventory) = &workflow.dolphin_inventory else {
                return None;
            };
            let game_id = identity.and_then(GameIdentityReport::verified_dolphin_game_id);
            let revision = identity.and_then(GameIdentityReport::verified_dolphin_revision);
            let matched = match_dolphin_inventory(inventory, game_id, revision);
            let strength = match matched.state {
                DolphinMatchState::ExactGameIdMatch
                | DolphinMatchState::ExactGameIdAndRevisionMatch
                | DolphinMatchState::MultipleIniFilesForGame => PreviewMatchStrength::VerifiedExact,
                DolphinMatchState::RevisionMismatch => PreviewMatchStrength::Ambiguous,
                _ => PreviewMatchStrength::Unsupported,
            };
            let source_items = matched
                .matching_files
                .iter()
                .filter_map(|path| {
                    let file = inventory.files.iter().find(|file| file.path == *path)?;
                    let relative = path.strip_prefix(&profile.configuration_path).ok()?;
                    Some(PreviewSourceItem {
                        adapter: PreviewAdapter::Dolphin,
                        source_path: path.clone(),
                        expected_source_digest: Some(file.sha256.clone()),
                        destination_relative_paths: vec![relative.to_path_buf()],
                        match_strength: strength,
                    })
                })
                .collect();
            Some((
                key,
                CheatPreviewWork::Shared(SharedPreviewRequest {
                    adapter: PreviewAdapter::Dolphin,
                    selected_archive: workflow.archive_path.clone(),
                    platform: workflow.platform.clone(),
                    identity: preview_identity(
                        workflow,
                        PreviewIdentityKind::DolphinGameId,
                        game_id,
                        revision,
                    ),
                    destination_root: profile.configuration_path.clone(),
                    source_items,
                }),
            ))
        }
        CheatEmulatorAdapter::RetroArch => {
            // The trusted-catalogue path no longer auto-materializes a whole
            // catalogue file. It goes through candidate selection and
            // individual cheat selection first, and its preview is produced
            // by `start_generated_cheat_preview` on an explicit request -
            // auto-previewing would re-run on every checkbox toggle and
            // would preview a file the user had not chosen the contents of.
            if workflow.source_mode == CheatSourceMode::ArchiveFsTrustedCatalogue {
                return None;
            }
            let selected = workflow.selected_profile_id.as_deref()?;
            let RetroArchProfilesState::Ready(discovery) = retroarch_profiles else {
                return None;
            };
            let profile = discovery
                .profiles
                .iter()
                .find(|profile| profile.eligible && profile.profile_id == selected)?;
            let root = profile.cheat_destination_root.as_ref()?;
            let destination_root = (!root.lossy).then(|| PathBuf::from(&root.display))?;
            let CheatStepResource::Ready(fetch) = &workflow.source_fetch else {
                return None;
            };
            if fetch.local_catalogue_path.lossy || fetch.immutable_snapshot_path.lossy {
                return None;
            }
            let platform = workflow.platform.clone()?;
            Some((
                key,
                CheatPreviewWork::RetroArch(RetroArchMaterializationRequest {
                    snapshot_root: PathBuf::from(&fetch.immutable_snapshot_path.display),
                    expected_snapshot_id: fetch.manifest.archive_sha256.clone(),
                    source_id: fetch.source.source_id.clone(),
                    selected_archive: workflow.archive_path.clone(),
                    archive_display_name: workflow.display_name.clone(),
                    archive_normalized_name: workflow.normalized_name.clone(),
                    platform,
                    region: workflow.region.clone(),
                    verified_loose_rom_sha256: identity
                        .and_then(GameIdentityReport::verified_loose_rom_sha256)
                        .map(str::to_owned),
                    destination_root,
                }),
            ))
        }
        // Xenia has no local bulk file inventory to match against - its
        // preview is built directly from the chosen provider candidate by
        // `start_xenia_install_preview`, never through this dispatcher.
        CheatEmulatorAdapter::Xenia | CheatEmulatorAdapter::Unsupported => None,
    }
}


pub(crate) fn identity_status_tone(status: IdentityStatus) -> widgets::StatusTone {
    match status {
        IdentityStatus::Verified => widgets::StatusTone::Success,
        IdentityStatus::Candidate | IdentityStatus::Deferred | IdentityStatus::Missing => {
            widgets::StatusTone::Pending
        }
        IdentityStatus::ResourceLimitReached | IdentityStatus::Ambiguous => {
            widgets::StatusTone::Warning
        }
        IdentityStatus::Invalid | IdentityStatus::Unsupported => widgets::StatusTone::Blocked,
    }
}


pub(crate) fn show_shared_game_identity(
    ui: &mut egui::Ui,
    workflow: &CheatWorkflowState,
    clipboard: &mut dyn ClipboardBackend,
) {
    widgets::section_header(
        ui,
        "Game recognition",
        Some("Bounded local disc metadata; candidates are never promoted to verified values."),
    );
    match &workflow.identity {
        CheatStepResource::NotLoaded => widgets::banner(
            ui,
            "Game version not confirmed",
            "Choose the game again to inspect its version.",
            widgets::StatusTone::Pending,
        ),
        CheatStepResource::Loading { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading bounded disc metadata in the background...");
            });
        }
        CheatStepResource::Failed(message) => widgets::banner(
            ui,
            "Could not recognise the game",
            message,
            widgets::StatusTone::Blocked,
        ),
        CheatStepResource::Ready((_, report)) => {
            widgets::card(ui, |ui| {
                if widgets::path_value(ui, "Selected archive", &report.archive_path) {
                    let _ = clipboard.set_text(report.archive_path.display().to_string());
                }
                ui.horizontal_wrapped(|ui| {
                    widgets::status_badge(ui, report.platform.label(), widgets::StatusTone::Info);
                    if report.format == IdentityImageFormat::LooseCartridgeRom {
                        widgets::status_badge(
                            ui,
                            "Media kind · Loose ROM",
                            widgets::StatusTone::Info,
                        );
                        ui.label(format!("{} bytes hashed", report.bytes_read));
                    } else {
                        ui.label(format!("Format: {:?}", report.format));
                        ui.label(format!("{} bytes read", report.bytes_read));
                    }
                    ui.label(format!(
                        "{} members · {} metadata paths",
                        report.archive_members_inspected, report.metadata_paths_inspected
                    ));
                });
            });
            for item in &report.evidence {
                widgets::card(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(item.kind.to_string());
                        widgets::status_badge(
                            ui,
                            item.status.to_string(),
                            identity_status_tone(item.status),
                        );
                        if let Some(value) = &item.value {
                            widgets::copyable_value(ui, "Value", value);
                        }
                    });
                    widgets::technical_details(
                        ui,
                        (
                            "identity_evidence_technical_provenance",
                            item.kind.to_string(),
                        ),
                        |ui| {
                            ui.label(format!("Method: {}", item.provenance.method));
                            ui.label(format!("Confidence: {:?}", item.confidence));
                            if let Some(index) = item.provenance.member_index {
                                ui.label(format!("ZIP member index: {index}"));
                            }
                            if let Some(member) = &item.provenance.member_path {
                                ui.label(format!(
                                    "ZIP member: {}",
                                    String::from_utf8_lossy(member)
                                ));
                            }
                            ui.label(&item.diagnostic);
                        },
                    );
                });
            }
            if !report.warnings.is_empty() {
                egui::CollapsingHeader::new(format!(
                    "Identity warnings ({})",
                    report.warnings.len()
                ))
                .default_open(false)
                .show(ui, |ui| {
                    for warning in &report.warnings {
                        ui.label(warning);
                    }
                });
            }
            let adapter_match = match workflow.adapter {
                CheatEmulatorAdapter::Pcsx2 => match &workflow.pcsx2_inventory {
                    CheatStepResource::Ready(inventory) => {
                        let result = match_pcsx2_inventory(
                            inventory,
                            report.verified_pcsx2_crc(),
                            Some(&workflow.display_name),
                        );
                        let (label, tone) = pcsx2_match_presentation(result.state);
                        Some((label, tone, result.reason))
                    }
                    _ => None,
                },
                CheatEmulatorAdapter::Dolphin => match &workflow.dolphin_inventory {
                    CheatStepResource::Ready(inventory) => {
                        let result = match_dolphin_inventory(
                            inventory,
                            report.verified_dolphin_game_id(),
                            report.verified_dolphin_revision(),
                        );
                        let (label, tone) = dolphin_match_presentation(result.state);
                        Some((label, tone, result.reason))
                    }
                    _ => None,
                },
                CheatEmulatorAdapter::RetroArch => None,
                CheatEmulatorAdapter::Xenia => None,
                CheatEmulatorAdapter::Unsupported => None,
            };
            widgets::card(ui, |ui| {
                ui.strong("Exact game match");
                if let Some((label, tone, reason)) = adapter_match {
                    ui.horizontal_wrapped(|ui| {
                        widgets::status_badge(ui, label, tone);
                        ui.label(reason);
                    });
                } else {
                    widgets::status_badge(
                        ui,
                        "Game files have not been checked yet",
                        widgets::StatusTone::Pending,
                    );
                }
            });
        }
    }
}


pub(crate) fn show_cheat_source_modes(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    _profiles: &RetroArchProfilesState,
) -> Option<CheatWorkflowAction> {
    widgets::section_header(
        ui,
        "Cheat source",
        Some("Choose where EmuWiz should look for cheats for this game."),
    );
    let show_existing = |ui: &mut egui::Ui, selected: bool| {
        let mut clicked = false;
        widgets::card(ui, |ui| {
            clicked = ui.radio(selected, "Existing RetroArch cheats").clicked();
            widgets::status_badge(ui, "Browse installed cheats", widgets::StatusTone::Info);
            ui.label("See cheats already stored in the selected RetroArch profile.");
        });
        clicked
    };
    let show_trusted = |ui: &mut egui::Ui, selected: bool| {
        let mut clicked = false;
        widgets::card(ui, |ui| {
            clicked = ui.radio(selected, "EmuWiz cheat catalogue").clicked();
            widgets::status_badge(ui, "Ready to search", widgets::StatusTone::Success);
            ui.label("Search the cheat catalogue bundled with EmuWiz.");
        });
        clicked
    };
    let existing_selected = workflow.source_mode == CheatSourceMode::ExistingRetroArchLibrary;
    let trusted_selected = workflow.source_mode == CheatSourceMode::ArchiveFsTrustedCatalogue;
    let (existing_clicked, trusted_clicked) = if ui.available_width() >= 760.0 {
        let mut existing_clicked = false;
        let mut trusted_clicked = false;
        ui.columns(2, |columns| {
            existing_clicked = show_existing(&mut columns[0], existing_selected);
            trusted_clicked = show_trusted(&mut columns[1], trusted_selected);
        });
        (existing_clicked, trusted_clicked)
    } else {
        (
            show_existing(ui, existing_selected),
            show_trusted(ui, trusted_selected),
        )
    };
    if existing_clicked && workflow.source_mode != CheatSourceMode::ExistingRetroArchLibrary {
        workflow.source_mode = CheatSourceMode::ExistingRetroArchLibrary;
        clear_cheat_candidate_state(workflow);
    } else if trusted_clicked && workflow.source_mode != CheatSourceMode::ArchiveFsTrustedCatalogue
    {
        workflow.source_mode = CheatSourceMode::ArchiveFsTrustedCatalogue;
        clear_cheat_candidate_state(workflow);
    }
    let show_planned = |ui: &mut egui::Ui, kind, body| {
        let (label, state) = import_source_presentation(kind);
        widgets::card(ui, |ui| {
            ui.strong(label);
            widgets::status_badge(ui, state, widgets::StatusTone::Pending);
            ui.label(body);
        });
    };
    widgets::technical_details(ui, "planned-cheat-source-modes", |ui| {
        let local_body =
            "A bounded local inspection backend is required before selection can be offered.";
        let remote_body = "User-defined remote sources are a future workflow.";
        if ui.available_width() >= 760.0 {
            ui.columns(2, |columns| {
                show_planned(
                    &mut columns[0],
                    ImportSourceKind::LocalUnverifiedSource,
                    local_body,
                );
                show_planned(
                    &mut columns[1],
                    ImportSourceKind::RemoteUnverifiedSource,
                    remote_body,
                );
            });
        } else {
            show_planned(ui, ImportSourceKind::LocalUnverifiedSource, local_body);
            show_planned(ui, ImportSourceKind::RemoteUnverifiedSource, remote_body);
        }
    });
    None
}


pub(crate) fn show_existing_retroarch_library(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    profiles: &RetroArchProfilesState,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<CheatWorkflowAction> {
    let selected_profile_id = workflow.selected_profile_id.as_deref();
    if selected_profile_id.is_none() {
        widgets::banner(
            ui,
            "Waiting for profile",
            "Choose an eligible RetroArch profile before inspecting its configured cheat directory.",
            widgets::StatusTone::Pending,
        );
        return None;
    }
    let selected_profile_id = selected_profile_id.unwrap();
    let destination = match profiles {
        RetroArchProfilesState::Ready(discovery) => discovery
            .profiles
            .iter()
            .find(|profile| profile.eligible && profile.profile_id == selected_profile_id)
            .and_then(|profile| profile.cheat_destination_root.as_ref()),
        _ => None,
    };
    let Some(destination) = destination else {
        widgets::banner(
            ui,
            "Destination unavailable",
            "The selected profile does not currently expose a safely resolved cheat destination.",
            widgets::StatusTone::Warning,
        );
        return None;
    };
    if destination.lossy {
        widgets::banner(
            ui,
            "Destination unavailable",
            "The configured path cannot be represented losslessly and will not be inspected.",
            widgets::StatusTone::Blocked,
        );
        return None;
    }
    let path = Path::new(&destination.display);
    widgets::card(ui, |ui| {
        widgets::status_strip(
            ui,
            &[
                ("Unverified local content", widgets::StatusTone::Warning),
                ("Modified by this workspace · No", widgets::StatusTone::Info),
            ],
        );
        if widgets::path_value(ui, "RetroArch cheat directory", path) {
            let _ = clipboard.set_text(destination.display.clone());
        }
        ui.label("Inspection is limited to reviewed platform aliases and plausible game filenames. Local files remain unverified compatibility evidence.");
    });

    if workflow.existing_library_profile_id.as_deref() != Some(selected_profile_id) {
        workflow.existing_library_profile_id = None;
        workflow.existing_library = CheatStepResource::NotLoaded;
    }
    match &workflow.existing_library {
        CheatStepResource::NotLoaded => Some(CheatWorkflowAction::InspectExistingLibrary),
        CheatStepResource::Loading { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Inspecting only relevant platform and game paths with fixed limits...");
            });
            None
        }
        CheatStepResource::Failed(message) => {
            widgets::banner(
                ui,
                "Inspection unavailable",
                message,
                widgets::StatusTone::Warning,
            );
            None
        }
        CheatStepResource::Ready(inspection) => {
            let (label, tone) = retroarch_library_state_presentation(inspection.state);
            let (match_label, match_tone) =
                retroarch_local_match_presentation(inspection.match_state);
            widgets::card(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    widgets::status_badge(ui, label, tone);
                    widgets::status_badge(ui, match_label, match_tone);
                    ui.label(format!(
                        "{} relevant cheat file{}",
                        inspection.approximate_cheat_file_count,
                        if inspection.approximate_cheat_file_count == 1 {
                            ""
                        } else {
                            "s"
                        }
                    ));
                    ui.label(format!(
                        "{} directories · {} files · {} bytes inspected",
                        inspection.directories_inspected,
                        inspection.entries_examined,
                        inspection.bytes_inspected
                    ));
                });
                for path in &inspection.matching_files {
                    if path.lossy {
                        ui.label(
                            "Matching path retained with non-UTF-8 identity (display is lossy).",
                        );
                    } else if widgets::path_value(ui, "Local candidate", Path::new(&path.display)) {
                        let _ = clipboard.set_text(path.display.clone());
                    }
                }
                if let Some(warning) = &inspection.warning {
                    ui.label(warning);
                }
            });
            None
        }
    }
}


pub(crate) fn retroarch_local_match_presentation(
    state: RetroArchLocalCheatMatchState,
) -> (&'static str, widgets::StatusTone) {
    match state {
        RetroArchLocalCheatMatchState::NotTargeted => {
            ("Not targeted", widgets::StatusTone::Pending)
        }
        RetroArchLocalCheatMatchState::NotFound => {
            ("No local cheat found", widgets::StatusTone::Pending)
        }
        RetroArchLocalCheatMatchState::Candidate => {
            ("Local filename candidate", widgets::StatusTone::Warning)
        }
        RetroArchLocalCheatMatchState::ExactLocalFile => {
            ("Exact local filename", widgets::StatusTone::Success)
        }
        RetroArchLocalCheatMatchState::Ambiguous => {
            ("Ambiguous local files", widgets::StatusTone::Warning)
        }
        RetroArchLocalCheatMatchState::LimitReached => {
            ("Inspection limit reached", widgets::StatusTone::Warning)
        }
        RetroArchLocalCheatMatchState::Unsafe => {
            ("Unsafe local path refused", widgets::StatusTone::Blocked)
        }
        RetroArchLocalCheatMatchState::Unavailable => {
            ("Local inspection unavailable", widgets::StatusTone::Pending)
        }
    }
}


pub(crate) fn retroarch_library_state_presentation(
    state: RetroArchCheatLibraryState,
) -> (&'static str, widgets::StatusTone) {
    match state {
        RetroArchCheatLibraryState::Missing => ("Directory missing", widgets::StatusTone::Pending),
        RetroArchCheatLibraryState::Available => ("Directory exists", widgets::StatusTone::Success),
        RetroArchCheatLibraryState::Inaccessible => {
            ("Directory inaccessible", widgets::StatusTone::Warning)
        }
        RetroArchCheatLibraryState::UnsafePath => {
            ("Unsafe path refused", widgets::StatusTone::Blocked)
        }
        RetroArchCheatLibraryState::LimitReached => {
            ("Inspection limit reached", widgets::StatusTone::Warning)
        }
    }
}


/// Step 2 of the cheat workflow: the built-in trusted source list, the
/// cached snapshot's provenance/digest/freshness, and retrieval
/// actions. Only the fixed trusted list is ever shown - there is no
/// user-supplied URL surface, and every retrieval runs on a background
/// thread through `fetch_retroarch_cheat_source`'s existing size/
/// digest/redirect protections.
pub(crate) fn show_cheat_workflow_step2(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    busy: bool,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(
        ui,
        "Available cheats",
        Some("EmuWiz will show the cheats available for the selected game."),
    );
    if workflow.selected_profile_id.is_none() {
        widgets::banner(
            ui,
            "Waiting for profile",
            "Choose an eligible RetroArch profile to continue.",
            widgets::StatusTone::Pending,
        );
        return None;
    }
    match &workflow.source_list {
        CheatStepResource::NotLoaded => {
            ui.label("Cheat sources have not been checked yet.");
            if widgets::action_button(
                ui,
                "Check cheat sources",
                widgets::ActionStyle::Primary,
                true,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::RefreshSources);
            }
        }
        CheatStepResource::Loading { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Checking cheat sources...");
            });
        }
        CheatStepResource::Failed(message) => {
            widgets::banner(
                ui,
                "Cheat sources need attention",
                message,
                widgets::StatusTone::Blocked,
            );
            if widgets::action_button(ui, "Retry", widgets::ActionStyle::Primary, true).clicked() {
                action = Some(CheatWorkflowAction::RefreshSources);
            }
        }
        CheatStepResource::Ready(list) => {
            for entry in &list.entries {
                let source = &entry.source;
                widgets::card(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if source.enabled {
                            let selected =
                                workflow.selected_source_id.as_deref() == Some(&source.source_id);
                            if ui.radio(selected, &source.display_name).clicked() {
                                workflow.selected_source_id = Some(source.source_id.clone());
                            }
                        } else {
                            ui.add_enabled(
                                false,
                                egui::Button::selectable(false, &source.display_name),
                            );
                            widgets::status_badge(ui, "Disabled", widgets::StatusTone::Pending);
                        }
                        if source.experimental {
                            widgets::status_badge(ui, "Experimental", widgets::StatusTone::Warning);
                        } else {
                            widgets::status_badge(ui, "Ready", widgets::StatusTone::Success);
                        }
                        widgets::status_badge(
                            ui,
                            cheat_freshness_label(entry.freshness),
                            cheat_freshness_tone(entry.freshness),
                        );
                    });
                    if workflow.selected_source_id.as_deref() == Some(&source.source_id) {
                        show_cheat_warnings_summary(
                            ui,
                            &entry.warnings,
                            ("cheat_source_warnings", &source.source_id),
                            clipboard,
                        );
                        widgets::technical_details(
                            ui,
                            ("cheat_source_technical_details", &source.source_id),
                            |ui| {
                                ui.label(format!("Permitted host: {}", source.permitted_host));
                                if let Some(fetched_at) = entry.fetched_at_unix_seconds {
                                    ui.label(format!(
                                        "Last retrieval: {}",
                                        format_unix_timestamp_utc(fetched_at as i64)
                                    ));
                                }
                                ui.label(&source.provenance);
                                widgets::copyable_value(ui, "Source identifier", &source.source_id);
                                widgets::copyable_value(ui, "Download URL", &source.download_url);
                                ui.label(format!("Trust status: {}", entry.trust_status));
                                if let Some(licence) = &source.licence_url {
                                    ui.label(format!("Licence: {licence}"));
                                }
                                if let Some(pinned) = &source.pinned_version {
                                    ui.label(format!("Pinned version: {pinned}"));
                                }
                                if let Some(version) = &entry.current_cached_version {
                                    ui.label(format!("Cached version: {version}"));
                                }
                                if let Some(digest) = &entry.archive_sha256 {
                                    widgets::copyable_value(ui, "SHA-256", digest);
                                }
                                if let Some(count) = entry.catalogue_file_count {
                                    ui.label(format!("Catalogue files: {count}"));
                                }
                                ui.label(format!(
                                    "Usable for setup: {}",
                                    if entry.setup_usable { "Yes" } else { "No" }
                                ));
                                for warning in &entry.warnings {
                                    ui.label(warning);
                                }
                            },
                        );
                    }
                });
                ui.add_space(6.0);
            }
            ui.add_space(4.0);
            let fetching = matches!(workflow.source_fetch, CheatStepResource::Loading { .. });
            let selected_entry = workflow.selected_source_id.as_deref().and_then(|id| {
                list.entries
                    .iter()
                    .find(|entry| entry.source.source_id == id)
            });
            ui.horizontal_wrapped(|ui| {
                let can_fetch =
                    !busy && !fetching && selected_entry.is_some_and(|entry| entry.source.enabled);
                if widgets::action_button(
                    ui,
                    "Open Cheat Sources",
                    widgets::ActionStyle::Primary,
                    can_fetch,
                )
                .clicked()
                {
                    action = Some(CheatWorkflowAction::ManageCatalogue);
                }
                let cached_available = selected_entry
                    .is_some_and(|entry| entry.freshness != CheatSourceFreshness::Missing);
                if widgets::action_button(
                    ui,
                    "Use saved catalogue",
                    widgets::ActionStyle::Secondary,
                    !busy && !fetching && cached_available,
                )
                .clicked()
                {
                    action = Some(CheatWorkflowAction::UseCachedSnapshot);
                }
                if widgets::action_button(
                    ui,
                    "Refresh source list",
                    widgets::ActionStyle::Quiet,
                    !fetching,
                )
                .clicked()
                {
                    action = Some(CheatWorkflowAction::RefreshSources);
                }
            });
            egui::CollapsingHeader::new("Advanced retrieval options")
                .default_open(false)
                .show(ui, |ui| {
                    ui.checkbox(
                        &mut workflow.fetch_force_refresh,
                        "Force a full refresh instead of reusing a fresh cache",
                    );
                });
        }
    }
    match &workflow.source_fetch {
        CheatStepResource::NotLoaded => {}
        CheatStepResource::Loading { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Retrieving catalogue...");
            });
        }
        CheatStepResource::Failed(message) => {
            widgets::banner(
                ui,
                "Cheat sources need attention",
                message,
                widgets::StatusTone::Blocked,
            );
        }
        CheatStepResource::Ready(result) => {
            widgets::card(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    widgets::status_badge(ui, "Catalogue ready", widgets::StatusTone::Success);
                    widgets::status_badge(
                        ui,
                        cheat_freshness_label(result.freshness),
                        if result.stale {
                            widgets::StatusTone::Warning
                        } else {
                            widgets::StatusTone::Success
                        },
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{} valid cheats",
                            result.manifest.valid_cheat_count
                        ))
                        .strong(),
                    );
                    ui.label(format!(
                        "Retrieved {}",
                        format_unix_timestamp_utc(result.manifest.fetched_at_unix_seconds as i64)
                    ));
                });
                ui.label(cheat_fetch_status_label(result.status));
                if result.stale {
                    widgets::banner(
                        ui,
                        "Stale snapshot",
                        "The cached catalogue remains usable for inspection, but should be updated when online.",
                        widgets::StatusTone::Warning,
                    );
                }
                show_cheat_warnings_summary(
                    ui,
                    &result.warnings,
                    (
                        "cheat_fetch_result_warnings",
                        &result.manifest.archive_sha256,
                    ),
                    clipboard,
                );
                widgets::technical_details(
                    ui,
                    (
                        "cheat_catalogue_technical_details",
                        &result.manifest.archive_sha256,
                    ),
                    |ui| {
                        widgets::copyable_value(ui, "SHA-256", &result.manifest.archive_sha256);
                        widgets::copyable_value(
                            ui,
                            "Catalogue path",
                            &result.local_catalogue_path.display,
                        );
                        widgets::copyable_value(
                            ui,
                            "Immutable snapshot",
                            &result.immutable_snapshot_path.display,
                        );
                        for warning in &result.warnings {
                            ui.label(warning);
                        }
                    },
                );
            });
        }
    }
    action
}


/// Why an exact-archive Cheats & Mods entry point is unavailable. Profile
/// readiness is deliberately not a navigation gate: the full page owns
/// profile discovery and truthfully presents blocked states.
pub(crate) fn cheat_entry_blocker(
    selected_archive: Option<&Path>,
    selected_count: usize,
    live_records: Option<&[ArchiveRecord]>,
    _profiles: &RetroArchProfilesState,
) -> Option<&'static str> {
    let Some(path) = selected_archive else {
        return Some("Select exactly one archive in the Library first.");
    };
    if selected_count > 1 {
        return Some(
            "RetroArch cheat setup works on exactly one archive; reduce the selection to one.",
        );
    }
    let Some(records) = live_records else {
        return Some("The live snapshot is still loading.");
    };
    if !records
        .iter()
        .any(|record| record.mount_plan.archive.path == path)
    {
        return Some("The selected archive is no longer present in the live snapshot.");
    }
    None
}


/// The eligible profile IDs in a discovery, in discovery order.
pub(crate) fn eligible_profile_ids(discovery: &RetroArchCheatSetupDiscovery) -> Vec<&str> {
    discovery
        .profiles
        .iter()
        .filter(|profile| profile.eligible)
        .map(|profile| profile.profile_id.as_str())
        .collect()
}


pub(crate) fn eligible_pcsx2_profile_ids(discovery: &Pcsx2ProfileDiscovery) -> Vec<&str> {
    discovery
        .profiles
        .iter()
        .filter(|profile| profile.eligible)
        .map(|profile| profile.profile_id.as_str())
        .collect()
}


/// Resolves the exact cheats directory for the currently selected PCSX2
/// profile, or `None` when it cannot be confidently identified (profile not
/// yet scanned, no longer eligible, or with no safe normal cheats
/// directory). The GUI must never silently substitute another directory or
/// proceed to install when this returns `None`.
pub(crate) fn resolved_pcsx2_cheats_directory(
    profiles: &Pcsx2ProfilesState,
    profile_id: &str,
) -> Option<PathBuf> {
    let Pcsx2ProfilesState::Ready(discovery) = profiles else {
        return None;
    };
    discovery
        .profiles
        .iter()
        .find(|profile| profile.profile_id == profile_id)
        .and_then(archivefs_core::patch_manager::pcsx2_cheats_directory)
        .map(Path::to_path_buf)
}


pub(crate) fn eligible_dolphin_profile_ids(discovery: &DolphinProfileDiscovery) -> Vec<&str> {
    discovery
        .profiles
        .iter()
        .filter(|profile| profile.eligible)
        .map(|profile| profile.profile_id.as_str())
        .collect()
}


/// Standard roots are rediscovered from current filesystem/runtime evidence;
/// they are not reintroduced as user-confirmed overrides from profile memory.
pub(crate) fn is_dolphin_standard_fallback_root(root: &Path) -> bool {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return false;
    };
    root == home.join(".config/dolphin-emu")
        || root == home.join(".var/app/org.DolphinEmu.dolphin-emu/data/dolphin-emu")
        || root == home.join(".var/app/org.DolphinEmu.dolphin-emu/config/dolphin-emu")
}


/// Rebinds an already-fetched Dolphin result to the newly selected local
/// profile. Provider loading and profile discovery run concurrently, so
/// either can finish first; without this reconciliation a provider result
/// that won the race stayed `Ready` but had no beginner selection until a
/// manual refresh.
pub(crate) fn reconcile_dolphin_provider_selection(
    workflow: &mut CheatWorkflowState,
    discovery: &DolphinProfileDiscovery,
) {
    let Some(profile_id) = workflow.selected_dolphin_profile_id.as_deref() else {
        workflow.dolphin_provider_selection = None;
        return;
    };
    let Some(configuration_path) = discovery
        .profiles
        .iter()
        .find(|profile| profile.eligible && profile.profile_id == profile_id)
        .map(|profile| profile.configuration_path.clone())
    else {
        workflow.dolphin_provider_selection = None;
        return;
    };
    bind_dolphin_provider_to_configuration(workflow, &configuration_path);
}


pub(crate) fn bind_dolphin_provider_to_configuration(
    workflow: &mut CheatWorkflowState,
    configuration_path: &Path,
) {
    let CheatStepResource::Ready(fetch) = &workflow.dolphin_provider else {
        return;
    };
    match load_dolphin_destination(configuration_path, &fetch.result.game_id) {
        Ok(destination) => {
            let selection =
                DolphinProviderCodeSelection::from_provider(&fetch.result, &destination);
            workflow.dolphin_provider_selection = Some(DolphinProviderSelectionState {
                destination,
                selection,
            });
            workflow.dolphin_destination_error = None;
        }
        Err(error) => {
            workflow.dolphin_provider_selection = None;
            workflow.dolphin_destination_error = Some(error.to_string());
        }
    }
}


/// Step 1 of the cheat workflow: archive identity plus explicit profile
/// selection. Renders from the shared `RetroArchProfilesState` (the
/// same discovery the Settings page shows) and mutates only the
/// workflow's own `selected_profile_id`. A selection that no longer
/// exists in the current discovery (profile vanished on rescan) is
/// cleared here rather than silently kept.
pub(crate) fn show_cheat_workflow_step1(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    profiles: &RetroArchProfilesState,
    busy: bool,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    widgets::card(ui, |ui| {
        show_cheat_activation_status(ui, "RetroArch", CheatActivationReadiness::Unknown);
    });
    widgets::section_header(
        ui,
        "Choose a RetroArch profile",
        Some("Choose where EmuWiz should look for and save cheats for this game."),
    );
    widgets::section_header(ui, "RetroArch profile", None);
    match profiles {
        RetroArchProfilesState::NotScanned => {
            ui.label("Profiles have not been scanned yet.");
        }
        RetroArchProfilesState::Scanning { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Scanning for RetroArch profiles...");
            });
        }
        RetroArchProfilesState::Error(message) => {
            widgets::banner(
                ui,
                "RetroArch profiles need attention",
                message,
                widgets::StatusTone::Warning,
            );
        }
        RetroArchProfilesState::Ready(discovery) => {
            let eligible = eligible_profile_ids(discovery);
            if let Some(selected) = workflow.selected_profile_id.clone()
                && !eligible.contains(&selected.as_str())
            {
                workflow.selected_profile_id = None;
                workflow.existing_library_profile_id = None;
                workflow.existing_library = CheatStepResource::NotLoaded;
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "The previously selected profile is no longer eligible; choose again.",
                );
            }
            if eligible.is_empty() {
                widgets::banner(
                    ui,
                    "No eligible profile",
                    "The discovered profiles are blocked. Review their concise blocker summaries below or rescan after correcting RetroArch configuration.",
                    widgets::StatusTone::Blocked,
                );
            } else if eligible.len() > 1 && workflow.selected_profile_id.is_none() {
                ui.label(format!(
                    "{} eligible profiles were found. EmuWiz never silently picks \
                     between them — choose one explicitly.",
                    eligible.len()
                ));
            }
            for profile in &discovery.profiles {
                widgets::card(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if profile.eligible {
                            let selected = workflow.selected_profile_id.as_deref()
                                == Some(&profile.profile_id);
                            let label = format!(
                                "RetroArch — {} ({})",
                                profile_kind_label(&profile.installation_type),
                                profile_scope_label(&profile.scope)
                            );
                            if ui.radio(selected, label).clicked() {
                                workflow.selected_profile_id = Some(profile.profile_id.clone());
                                workflow.existing_library_profile_id = None;
                                workflow.existing_library = CheatStepResource::NotLoaded;
                                // A different profile has a different cheat
                                // directory, so the destination - and every
                                // stage computed from it - must be recomputed
                                // rather than carried across.
                                clear_cheat_candidate_state(workflow);
                            }
                        } else {
                            ui.add_enabled(
                                false,
                                egui::Button::selectable(
                                    false,
                                    format!(
                                        "RetroArch — {} ({})",
                                        profile_kind_label(&profile.installation_type),
                                        profile_scope_label(&profile.scope)
                                    ),
                                ),
                            );
                            widgets::status_badge(ui, "Blocked", profile_presentation_tone(false));
                        }
                    });
                    if !profile.blockers.is_empty() {
                        ui.label("This RetroArch setup cannot be used until its configuration is corrected.");
                    }
                    widgets::technical_details(
                        ui,
                        ("retroarch_profile_blockers", &profile.profile_id),
                        |ui| {
                            ui.label(format!("Profile ID: {}", profile.profile_id));
                            for blocker in &profile.blockers {
                                ui.label(format!("{} — {}", blocker.code, blocker.detail));
                            }
                        },
                    );
                });
                ui.add_space(6.0);
            }
        }
    }
    ui.add_space(4.0);
    if widgets::action_button(ui, "Rescan profiles", widgets::ActionStyle::Quiet, !busy).clicked() {
        action = Some(CheatWorkflowAction::RescanProfiles);
    }
    action
}

