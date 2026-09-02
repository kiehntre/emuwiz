//! Cheats & Mods presentation: the shared cheat-preview renderer used by
//! every emulator-specific workflow (RetroArch, PCSX2, Dolphin, Xenia),
//! match-confidence/proposed-action wording, candidate-selection UI, and
//! the top-level Cheats & Mods page itself. Extracted verbatim from
//! `main.rs` (2026-08-22, GUI extraction pass 2).
//!
//! Deliberately excludes cheat-preview *request-building* logic
//! (`cheat_preview_key`, `build_cheat_preview_request`, `preview_identity`)
//! and the per-emulator workflow-step business logic that follows this
//! code in the old `main.rs` (profile eligibility, directory resolution,
//! provider reconciliation) - those decide what a preview *is*, not how
//! one renders, and stayed in `main.rs`.

use super::*;

/// Phase 6: the human-facing primary label for a preview entry's overall
/// state - what actually renders in the card's main badge now. Keeps
/// `preview_state_label` itself (the precise, unmodified original names)
/// available unchanged for `technical_details`, so nothing here collapses
/// the real, distinct states this represents - only their presentation.
pub(crate) fn preview_state_human_label(state: PreviewState) -> &'static str {
    match state {
        PreviewState::InstallNew => "Ready to install",
        PreviewState::AlreadyInstalled => "Already installed",
        PreviewState::ReplaceDifferent => "Will replace an existing file",
        PreviewState::Conflict => "There's a conflict here",
        PreviewState::Ambiguous => "We're not sure which one applies",
        PreviewState::NotEligible => "Not available for this game",
        PreviewState::Unsupported => "Not supported here",
        PreviewState::UnsafeDestination => "Blocked for safety",
        PreviewState::DestinationUnavailable => "Can't reach the destination right now",
        PreviewState::SourceUnavailable => "Can't reach the source file right now",
        PreviewState::IdentityUnavailable => "Couldn't verify which game this is",
        PreviewState::ResourceLimitReached => "Too many items to check right now",
    }
}

/// Phase 6: translates `entry.proposed_action` into a sentence instead of
/// `format!("Proposed action: {:?}")`'s raw enum Debug output - the
/// internal `PreviewProposedAction` enum itself is unchanged.
pub(crate) fn proposed_action_human_label(action: PreviewProposedAction) -> &'static str {
    match action {
        PreviewProposedAction::Install => "Install this cheat file",
        PreviewProposedAction::Skip => "No change needed",
        PreviewProposedAction::Replace => "Replace the existing cheat file",
        PreviewProposedAction::Blocked => "Needs review before anything is changed",
    }
}

pub(crate) fn preview_state_label(state: PreviewState) -> &'static str {
    match state {
        PreviewState::InstallNew => "Install new",
        PreviewState::AlreadyInstalled => "Already installed",
        PreviewState::ReplaceDifferent => "Replace different",
        PreviewState::Conflict => "Conflict",
        PreviewState::Ambiguous => "Ambiguous",
        PreviewState::NotEligible => "Not eligible",
        PreviewState::Unsupported => "Unsupported",
        PreviewState::UnsafeDestination => "Unsafe destination",
        PreviewState::DestinationUnavailable => "Destination unavailable",
        PreviewState::SourceUnavailable => "Source unavailable",
        PreviewState::IdentityUnavailable => "Identity unavailable",
        PreviewState::ResourceLimitReached => "Resource limit reached",
    }
}

pub(crate) fn preview_state_tone(state: PreviewState) -> widgets::StatusTone {
    match state {
        PreviewState::InstallNew | PreviewState::AlreadyInstalled => widgets::StatusTone::Success,
        PreviewState::ReplaceDifferent | PreviewState::ResourceLimitReached => {
            widgets::StatusTone::Warning
        }
        PreviewState::Conflict | PreviewState::Ambiguous | PreviewState::UnsafeDestination => {
            widgets::StatusTone::Blocked
        }
        _ => widgets::StatusTone::Pending,
    }
}

/// The user-facing label, tone, and one-sentence explanation for a
/// candidate's `PreviewMatchStrength` - the coarse match-confidence
/// category the matching engine actually produces today. This is the
/// full granularity available: the engine does not currently report
/// which specific piece of evidence (title, serial, CRC, region,
/// filename) drove a match, only the resulting confidence tier, so the
/// explanation stays honest about that rather than inventing detail the
/// engine doesn't have.
/// Phase 6: the primary label is now a human phrase, not a classification
/// word - "Do not overstate confidence" applies both ways here (Strong
/// still reads as short of independently verified, Candidate still reads
/// as needing a check). The precise original term (`Verified exact
/// match`/`Strong match`/`Candidate match`/`Ambiguous`/`Unsupported`)
/// moves into `technical_details` at the one call site - see
/// `preview_match_strength_technical_label` - so it isn't lost, only
/// no longer the primary presentation.
pub(crate) fn preview_match_strength_presentation(
    strength: PreviewMatchStrength,
) -> (&'static str, widgets::StatusTone, &'static str) {
    match strength {
        PreviewMatchStrength::VerifiedExact => (
            "Verified match",
            widgets::StatusTone::Success,
            "This was independently verified against the exact file - as confident as EmuWiz \
             gets.",
        ),
        PreviewMatchStrength::Strong => (
            "Confident match",
            widgets::StatusTone::Success,
            "This looks like a confident match, automatically paired but not independently \
             verified.",
        ),
        PreviewMatchStrength::Candidate => (
            "Possible match",
            widgets::StatusTone::Warning,
            "We found a possible match, but it needs checking before installing - it hasn't \
             been confirmed exact.",
        ),
        PreviewMatchStrength::Ambiguous => (
            "Not sure which one",
            widgets::StatusTone::Blocked,
            "We're not sure which game this file belongs to - more than one option could \
             apply, so EmuWiz won't guess between them.",
        ),
        PreviewMatchStrength::Unsupported => (
            "Not supported",
            widgets::StatusTone::Blocked,
            "No automatic matching is possible for this combination of game and file.",
        ),
    }
}

/// The precise classification word `preview_match_strength_presentation`
/// used to show as its primary label, preserved for `technical_details`.
pub(crate) fn preview_match_strength_technical_label(
    strength: PreviewMatchStrength,
) -> &'static str {
    match strength {
        PreviewMatchStrength::VerifiedExact => "Verified exact match",
        PreviewMatchStrength::Strong => "Strong match",
        PreviewMatchStrength::Candidate => "Candidate match",
        PreviewMatchStrength::Ambiguous => "Ambiguous",
        PreviewMatchStrength::Unsupported => "Unsupported",
    }
}

pub(crate) fn destination_state_label(state: PreviewDestinationState) -> &'static str {
    match state {
        PreviewDestinationState::Missing => "Missing",
        PreviewDestinationState::RegularFileIdentical => "Regular file · identical",
        PreviewDestinationState::RegularFileDifferent => "Regular file · different",
        PreviewDestinationState::Directory => "Directory",
        PreviewDestinationState::Symlink => "Symlink",
        PreviewDestinationState::SpecialFile => "Special file",
        PreviewDestinationState::Inaccessible => "Inaccessible",
        PreviewDestinationState::ChangedDuringInspection => "Changed during inspection",
        PreviewDestinationState::Unavailable => "Unavailable",
    }
}

/// Stages 4-6 of the Cheats & Mods workflow: candidate matches, the
/// evidence for the chosen one, and its individual cheats.
///
/// Only the stage the user is actually on is expanded. Before a candidate
/// is chosen this is a list; afterwards it is that candidate's identity,
/// evidence, and cheat picker, with one control to go back to the list.
pub(crate) fn show_cheat_candidate_stages(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    widgets::section_header(
        ui,
        "Candidate matches",
        Some("Cheat files from the trusted catalogue that could belong to this archive."),
    );

    match &workflow.candidates {
        CheatStepResource::NotLoaded => {
            widgets::card(ui, |ui| {
                ui.label(
                    "Select a RetroArch profile and retrieve the trusted catalogue, then \
                     EmuWiz matches this exact archive against it.",
                );
                if show_find_matching_cheats_button(ui).clicked() {
                    action = Some(CheatWorkflowAction::MatchCandidates);
                }
            });
            return action;
        }
        CheatStepResource::Loading { .. } => {
            widgets::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Matching this archive against the trusted catalogue…");
                });
            });
            return action;
        }
        CheatStepResource::Failed(message) => {
            if let Some(reason) = message.strip_prefix(CHEAT_MATCH_BLOCKED_PREFIX) {
                widgets::banner(ui, "Matching blocked", reason, widgets::StatusTone::Pending);
                widgets::card(ui, |ui| {
                    if widgets::action_button(ui, "Try again", widgets::ActionStyle::Primary, true)
                        .clicked()
                    {
                        action = Some(CheatWorkflowAction::MatchCandidates);
                    }
                });
            } else {
                widgets::banner(ui, "Matching failed", message, widgets::StatusTone::Blocked);
                widgets::card(ui, |ui| {
                    if widgets::action_button(
                        ui,
                        "Try again",
                        widgets::ActionStyle::Secondary,
                        true,
                    )
                    .clicked()
                    {
                        action = Some(CheatWorkflowAction::MatchCandidates);
                    }
                });
            }
            return action;
        }
        CheatStepResource::Ready(_) => {}
    }

    // Stage 5 and 6: a candidate is chosen, so the list collapses to a
    // single "change" control and the page moves on to its contents.
    if workflow.candidate_selection.is_some() {
        action = show_selected_candidate(ui, workflow, clipboard).or(action);
        return action;
    }

    if let Some(message) = workflow.candidate_load_error.clone() {
        widgets::banner(
            ui,
            "That candidate could not be opened",
            &message,
            widgets::StatusTone::Blocked,
        );
    }

    let CheatStepResource::Ready(stage) = &workflow.candidates else {
        return action;
    };
    if stage.list.is_empty() {
        widgets::banner(
            ui,
            "No cheats found for this game",
            "Try another cheat source or check the selected game's platform.",
            widgets::StatusTone::Pending,
        );
        return action;
    }

    if stage.list.truncated {
        widgets::card(ui, |ui| {
            ui.label(format!(
                "Showing the {} strongest of {} matching cheat files. Search to narrow the list.",
                stage.list.candidates.len(),
                stage.list.total_matched
            ));
            ui.text_edit_singleline(&mut workflow.candidate_query);
            if widgets::action_button(ui, "Search", widgets::ActionStyle::Secondary, true).clicked()
            {
                action = Some(CheatWorkflowAction::MatchCandidates);
            }
        });
    }

    let CheatStepResource::Ready(stage) = &workflow.candidates else {
        return action;
    };
    for candidate in &stage.list.candidates {
        widgets::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let (label, tone) = candidate_classification_presentation(candidate.classification);
                widgets::status_badge(ui, label, tone);
                ui.strong(&candidate.display_name);
                if let Some(platform) = &candidate.platform {
                    ui.label(platform);
                }
                if let Some(region) = &candidate.region {
                    ui.label(region);
                }
                ui.label(format!("{} cheats", candidate.cheat_count));
            });
            widgets::technical_details(
                ui,
                (
                    "cheat_candidate_source",
                    candidate.catalogue_relative_path.as_str(),
                ),
                |ui| {
                    ui.label(format!(
                        "Catalogue item: {}",
                        candidate.catalogue_relative_path
                    ));
                    show_candidate_evidence(ui, candidate);
                },
            );
            if candidate.manually_selectable {
                if widgets::action_button(
                    ui,
                    "Use this cheat file",
                    if candidate.classification == CheatCandidateClassification::Ambiguous {
                        widgets::ActionStyle::Secondary
                    } else {
                        widgets::ActionStyle::Primary
                    },
                    true,
                )
                .clicked()
                {
                    action = Some(CheatWorkflowAction::SelectCandidate(
                        candidate.catalogue_relative_path.clone(),
                    ));
                }
            } else {
                ui.label(candidate_block_reason(candidate.classification));
            }
        });
    }
    action
}

/// The stage-4 primary action. Extracted into its own function (rather
/// than inlined at its one call site) so a test can render and click this
/// exact widget directly - see `find_matching_cheat_files_button_*` below.
pub(crate) fn show_find_matching_cheats_button(ui: &mut egui::Ui) -> egui::Response {
    widgets::action_button(
        ui,
        "Find matching cheat files",
        widgets::ActionStyle::Primary,
        true,
    )
}

pub(crate) fn candidate_classification_presentation(
    classification: CheatCandidateClassification,
) -> (&'static str, widgets::StatusTone) {
    match classification {
        CheatCandidateClassification::VerifiedExact => {
            ("Verified exact", widgets::StatusTone::Success)
        }
        CheatCandidateClassification::Strong => ("Strong match", widgets::StatusTone::Info),
        CheatCandidateClassification::Ambiguous => ("Ambiguous", widgets::StatusTone::Warning),
        CheatCandidateClassification::Weak => ("Weak match", widgets::StatusTone::Warning),
        CheatCandidateClassification::CrossPlatform => {
            ("Different platform", widgets::StatusTone::Blocked)
        }
        CheatCandidateClassification::Unsupported => ("Unsupported", widgets::StatusTone::Blocked),
    }
}

pub(crate) fn candidate_block_reason(classification: CheatCandidateClassification) -> &'static str {
    match classification {
        CheatCandidateClassification::CrossPlatform => {
            "This cheat file is for a different system, so it can never be installed for this archive."
        }
        CheatCandidateClassification::Unsupported => {
            "This cheat file targets another emulator or did not parse cleanly, so it cannot be installed."
        }
        _ => "",
    }
}

/// Why this candidate matched, in the catalogue's own terms. Every line
/// corresponds to a comparison that was actually made.
pub(crate) fn show_candidate_evidence(ui: &mut egui::Ui, candidate: &CheatCandidate) {
    if candidate.evidence.is_empty() {
        return;
    }
    for evidence in &candidate.evidence {
        let tone = if evidence.kind.is_supporting() {
            widgets::StatusTone::Success
        } else {
            widgets::StatusTone::Warning
        };
        ui.horizontal_wrapped(|ui| {
            widgets::status_badge(ui, evidence.kind.code(), tone);
            ui.label(&evidence.detail);
        });
    }
}

/// Stage 5 (evidence for the chosen candidate) and stage 6 (its cheats).
pub(crate) fn show_selected_candidate(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    let Some(selection) = workflow.candidate_selection.as_ref() else {
        return action;
    };
    let candidate = &selection.candidate;

    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            let (label, tone) = candidate_classification_presentation(candidate.classification);
            widgets::status_badge(ui, label, tone);
            ui.strong(&candidate.display_name);
            if widgets::action_button(
                ui,
                "Choose a different cheat file",
                widgets::ActionStyle::Quiet,
                true,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::ClearCandidateChoice);
            }
        });
        if widgets::path_value(ui, "Catalogue file", &selection.loaded.absolute_path) {
            let _ = clipboard.set_text(selection.loaded.absolute_path.display().to_string());
        }
        widgets::copyable_value(ui, "Catalogue file SHA-256", &selection.loaded.digest);
        show_candidate_evidence(ui, candidate);
    });

    ui.add_space(theme::SECTION_GAP);
    action = show_cheat_entry_picker(ui, workflow).or(action);
    action
}

/// Stage 6: the individual cheats, in catalogue order.
pub(crate) fn show_cheat_entry_picker(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    let Some(selection) = workflow.candidate_selection.as_ref() else {
        return action;
    };
    let selected_count = selection.selection.selected_count();
    let selectable_count = selection.selection.selectable_count();
    let blocked_count = selection.selection.blocked_count();
    let document_warnings: Vec<String> = selection
        .loaded
        .document
        .warnings
        .iter()
        .map(|warning| warning.detail.clone())
        .collect();

    widgets::section_header(
        ui,
        "Cheats to install",
        Some(
            "Ticked cheats are written into the installed file. 'Active' decides whether RetroArch turns one on as soon as it loads.",
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
                action = Some(CheatWorkflowAction::SelectAllCheats);
            }
            if widgets::action_button(
                ui,
                "Clear all",
                widgets::ActionStyle::Quiet,
                selected_count > 0,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::ClearAllCheats);
            }
        });
        for warning in &document_warnings {
            widgets::status_badge(ui, warning.as_str(), widgets::StatusTone::Warning);
        }
    });

    let Some(selection) = workflow.candidate_selection.as_ref() else {
        return action;
    };
    for entry in &selection.selection.entries {
        widgets::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if entry.selectable {
                    let mut selected = entry.selected;
                    if ui.checkbox(&mut selected, &entry.description).changed() {
                        action = Some(CheatWorkflowAction::ToggleCheatSelected {
                            index: entry.source_index,
                            selected,
                        });
                    }
                } else {
                    ui.add_enabled(false, egui::Checkbox::new(&mut false, &entry.description));
                    widgets::status_badge(ui, "Unavailable", widgets::StatusTone::Blocked);
                }
                if entry.selectable && entry.selected {
                    let mut enabled = entry.enabled;
                    if ui.checkbox(&mut enabled, "Active on load").changed() {
                        action = Some(CheatWorkflowAction::ToggleCheatEnabled {
                            index: entry.source_index,
                            enabled,
                        });
                    }
                }
            });
            for warning in &entry.warnings {
                ui.label(warning);
            }
        });
    }

    let Some(selection) = workflow.candidate_selection.as_ref() else {
        return action;
    };
    ui.add_space(theme::SECTION_GAP);
    widgets::card(ui, |ui| {
        if selection.selection.can_apply() {
            if widgets::action_button(
                ui,
                "Preview the installed file",
                widgets::ActionStyle::Primary,
                !matches!(workflow.preview, CheatStepResource::Loading { .. }),
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::BuildInstallPreview);
            }
        } else {
            widgets::banner(
                ui,
                "Choose at least one cheat",
                "Nothing can be previewed or installed until at least one usable cheat is selected.",
                widgets::StatusTone::Pending,
            );
        }
    });
    action
}

/// Stage 7: exactly what installing would write, before anything is
/// written.
pub(crate) fn show_generated_install_preview(
    ui: &mut egui::Ui,
    generated: &GeneratedCheatInstall,
    clipboard: &mut dyn ClipboardBackend,
) {
    widgets::card(ui, |ui| {
        widgets::status_badge(ui, "Preview only", widgets::StatusTone::Info);
        ui.strong(format!("From: {}", generated.candidate_display_name));
        if widgets::path_value(ui, "Destination", &generated.destination.path) {
            let _ = clipboard.set_text(generated.destination.path.display().to_string());
        }
        ui.label(format!(
            "Filename taken from the {}; platform directory from the {}.",
            generated.destination.name_source.label(),
            generated.destination.platform_directory_source.label()
        ));
        ui.label(if generated.destination.replaces_existing {
            "A cheat file already exists at this path. Installing replaces it, and the existing file is backed up first."
        } else {
            "No file exists at this path yet. Installing creates it."
        });
        ui.label(format!(
            "{} cheat(s) will be written; {} of them start active.",
            generated.staged.selected_cheat_count, generated.staged.enabled_cheat_count
        ));
        widgets::copyable_value(ui, "New file SHA-256", &generated.staged.digest);
        widgets::technical_details(ui, "generated_cheat_file_contents", |ui| {
            ui.label("Exact file contents:");
            ui.code(&generated.staged.contents);
        });
    });
}

pub(crate) fn show_shared_cheat_preview(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    widgets::section_header(
        ui,
        "Shared preview",
        Some("Preview only. No files were changed."),
    );
    widgets::banner(
        ui,
        "Preview only. No files were changed.",
        "EmuWiz performs bounded local reads. A write can be offered only after an exact materialized source enters the reviewed transaction pipeline.",
        widgets::StatusTone::Info,
    );
    match &workflow.preview {
        CheatStepResource::NotLoaded => widgets::banner(
            ui,
            "Preview waiting",
            "A verified identity, selected profile, and inspected adapter source are required before a source-to-destination preview can run.",
            widgets::StatusTone::Pending,
        ),
        CheatStepResource::Loading { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Inspecting preview sources and destinations off the UI thread...");
            });
        }
        CheatStepResource::Failed(message) => widgets::banner(
            ui,
            "Preview worker failed",
            message,
            widgets::StatusTone::Blocked,
        ),
        CheatStepResource::Ready(response) => match &response.outcome {
            CheatPreviewOutcome::Failed(CheatPreviewFailure::Materialization(error))
                if error.kind == RetroArchMaterializationErrorKind::MatchingEntryExcluded =>
            {
                widgets::banner(
                    ui,
                    "Matching catalogue entry excluded",
                    &format!("{}. Nothing can be applied.", error.detail),
                    widgets::StatusTone::Warning,
                )
            }
            CheatPreviewOutcome::Failed(CheatPreviewFailure::Materialization(error))
                if error.kind == RetroArchMaterializationErrorKind::NoEligibleMatch =>
            {
                widgets::banner(
                    ui,
                    "No cheats found for this game",
                    "Try another cheat source or check the selected game's platform.",
                    widgets::StatusTone::Pending,
                )
            }
            CheatPreviewOutcome::Failed(CheatPreviewFailure::InstallPlan(error)) => {
                widgets::banner(
                    ui,
                    "Install preview blocked",
                    &error.detail,
                    widgets::StatusTone::Blocked,
                )
            }
            CheatPreviewOutcome::Failed(error) => widgets::banner(
                ui,
                "Preview unavailable",
                &error.to_string(),
                widgets::StatusTone::Blocked,
            ),
            CheatPreviewOutcome::Ready(report) => {
                if let Some(generated) = &response.generated {
                    show_generated_install_preview(ui, generated, clipboard);
                }
                if let Some(generated) = &response.dolphin_generated {
                    show_dolphin_generated_install_preview(ui, generated, clipboard);
                }
                if let Some(generated) = &response.xenia_generated {
                    show_xenia_generated_install_preview(ui, generated, clipboard);
                }
                if let Some(materialized) = &response.materialized {
                    widgets::technical_details(ui, "cheat_materialized_source", |ui| {
                        ui.label("Trusted source prepared for this exact preview.");
                        widgets::copyable_value(ui, "Snapshot ID", &materialized.snapshot_id);
                        if let CheatStepResource::Ready(fetch) = &workflow.source_fetch {
                            widgets::copyable_value(
                                ui,
                                "Upstream revision",
                                &fetch.manifest.resolved_revision,
                            );
                        }
                        if widgets::path_value(
                            ui,
                            "Immutable snapshot",
                            &materialized.snapshot_root,
                        ) {
                            let _ = clipboard
                                .set_text(materialized.snapshot_root.display().to_string());
                        }
                        ui.label(format!(
                            "{} exact local source item{}",
                            materialized.sources.len(),
                            if materialized.sources.len() == 1 {
                                ""
                            } else {
                                "s"
                            }
                        ));
                        ui.label(format!(
                            "Catalogue index: {:?} · {} indexed · {} excluded",
                            materialized.catalogue_index_state,
                            materialized.indexed_file_count,
                            materialized.excluded_file_count
                        ));
                    });
                }
                widgets::card(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        widgets::status_badge(ui, "Preview ready", widgets::StatusTone::Success);
                        ui.strong(format!("{} cheat file(s) checked", report.summary.entries));
                        ui.label(format!("{} ready to install", report.summary.install_new));
                        ui.label(format!(
                            "{} already installed",
                            report.summary.already_installed
                        ));
                        ui.label(format!(
                            "{} replace different",
                            report.summary.replace_different
                        ));
                        ui.label(format!("{} conflicts", report.summary.conflicts));
                        ui.label(format!("{} blocked", report.summary.blocked));
                    });
                    widgets::technical_details(ui, "cheat_preview_summary", |ui| {
                        ui.label(format!("Adapter: {:?}", report.adapter));
                        ui.label(format!(
                            "Hashed {} source and {} destination files · {} total bytes · {} paths inspected",
                            report.summary.source_files_hashed,
                            report.summary.destination_files_hashed,
                            report.summary.bytes_hashed,
                            report.summary.destination_paths_inspected
                        ));
                    });
                });
                for (index, entry) in report.entries.iter().enumerate() {
                    widgets::card(ui, |ui| {
                        // Primary presentation (Phase 6): what EmuWiz thinks,
                        // in plain language - state, eligibility, verified
                        // identity if any, match confidence, and what will
                        // happen. Every raw hash, path, precise enum name,
                        // and blocker/warning debug dump moves into
                        // `technical_details` below rather than being
                        // deleted - "Advanced users must still be able to
                        // inspect everything."
                        ui.horizontal_wrapped(|ui| {
                            widgets::status_badge(
                                ui,
                                preview_state_human_label(entry.state),
                                preview_state_tone(entry.state),
                            );
                            widgets::status_badge(
                                ui,
                                if entry.eligibility == PreviewEligibility::Eligible {
                                    "Eligible"
                                } else {
                                    "Blocked"
                                },
                                if entry.eligibility == PreviewEligibility::Eligible {
                                    widgets::StatusTone::Success
                                } else {
                                    widgets::StatusTone::Blocked
                                },
                            );
                            ui.strong(format!("Preview entry {}", index + 1));
                        });
                        if let Some(identity) = &entry.verified_identity {
                            widgets::copyable_value(ui, "Verified identity", identity);
                        } else {
                            ui.label("Verified identity: unavailable");
                        }
                        {
                            let (label, tone, explanation) =
                                preview_match_strength_presentation(entry.match_strength);
                            widgets::status_badge(ui, label, tone);
                            ui.label(explanation);
                        }
                        ui.label(format!(
                            "If you continue: {}",
                            proposed_action_human_label(entry.proposed_action)
                        ));
                        widgets::technical_details(
                            ui,
                            ("preview_entry_technical_detail", index),
                            |ui| {
                                ui.label(format!(
                                    "Match classification: {}",
                                    preview_match_strength_technical_label(entry.match_strength)
                                ));
                                ui.label(format!(
                                    "Entry state: {}",
                                    preview_state_label(entry.state)
                                ));
                                ui.label(format!("Proposed action: {:?}", entry.proposed_action));
                                if let Some(source) = &entry.source_path
                                    && widgets::path_value(ui, "Source item", source)
                                {
                                    let _ = clipboard.set_text(source.display().to_string());
                                }
                                if let Some(destination) = &entry.destination_path
                                    && widgets::path_value(ui, "Destination", destination)
                                {
                                    let _ = clipboard.set_text(destination.display().to_string());
                                }
                                ui.label(format!(
                                    "Current destination state: {}",
                                    destination_state_label(entry.destination_state)
                                ));
                                ui.label(format!(
                                    "Backup required: {} · explicit replacement permission \
                                     required: {}",
                                    if entry.backup_required { "Yes" } else { "No" },
                                    if entry.explicit_replacement_permission_required {
                                        "Yes"
                                    } else {
                                        "No"
                                    }
                                ));
                                if let Some(digest) = &entry.source_digest {
                                    widgets::copyable_value(ui, "Source SHA-256", digest);
                                }
                                if let Some(digest) = &entry.existing_destination_digest {
                                    widgets::copyable_value(ui, "Destination SHA-256", digest);
                                }
                                if widgets::path_value(
                                    ui,
                                    "Destination root",
                                    &entry.destination_root,
                                ) {
                                    let _ = clipboard
                                        .set_text(entry.destination_root.display().to_string());
                                }
                                if let Some(relative) = &entry.destination_relative_path {
                                    ui.label(format!("Relative path: {}", relative.display()));
                                }
                                for blocker in &entry.blockers {
                                    ui.label(format!(
                                        "Blocker: {:?}{}",
                                        blocker.kind,
                                        blocker
                                            .path
                                            .as_ref()
                                            .map(|path| format!(" · {}", path.display()))
                                            .unwrap_or_default()
                                    ));
                                }
                                for warning in &entry.warnings {
                                    ui.label(format!("Warning: {:?}", warning.kind));
                                }
                            },
                        );
                    });
                }
                if !report.conflicts.is_empty() {
                    egui::CollapsingHeader::new(format!(
                        "Conflict records ({})",
                        report.conflicts.len()
                    ))
                    .default_open(false)
                    .show(ui, |ui| {
                        for conflict in &report.conflicts {
                            ui.label(format!("{:?}", conflict.kind));
                        }
                    });
                }
                action = show_shared_transaction_readiness(
                    ui,
                    report,
                    response.materialized.is_some()
                        || response.generated.is_some()
                        || response.dolphin_generated.is_some()
                        || response.xenia_generated.is_some()
                        || response.pcsx2_generated.is_some()
                        || response.gamecube_gamehacking_generated.is_some(),
                    &mut workflow.transaction,
                    clipboard,
                );
            }
        },
    }
    action
}

pub(crate) fn show_shared_transaction_readiness(
    ui: &mut egui::Ui,
    report: &SharedPreviewReport,
    source_materialized: bool,
    transaction: &mut CheatTransactionState,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(
        ui,
        "Install selected cheats",
        Some("Review the changes before anything is written."),
    );
    let support = adapter_write_support(report.adapter);
    widgets::card(ui, |ui| {
        match support {
            SharedAdapterWriteSupport::ApplyAndRollback => {
                widgets::status_badge(ui, "Ready to install", widgets::StatusTone::Success);
                ui.label(
                    "EmuWiz will back up changed files, check the result, and keep the information needed to undo this install.",
                );
            }
            SharedAdapterWriteSupport::PreviewOnlySourceNotMaterialized => {
                widgets::status_badge(ui, "Preview only", widgets::StatusTone::Pending);
                ui.label(
                    "EmuWiz can show these files, but it cannot safely install them for this emulator yet.",
                );
            }
        }
        let actionable = report
            .entries
            .iter()
            .filter(|entry| {
                entry.eligibility == PreviewEligibility::Eligible
                    && matches!(
                        entry.proposed_action,
                        archivefs_core::patch_manager::PreviewProposedAction::Install
                            | archivefs_core::patch_manager::PreviewProposedAction::Replace
                    )
            })
            .count();
        widgets::technical_details(ui, "shared_transaction_contract", |ui| {
            ui.horizontal_wrapped(|ui| {
                for stage in [
                    "1 Preview",
                    "2 Review",
                    "3 Confirm",
                    "4 Apply",
                    "5 Verify",
                    "6 Result",
                ] {
                    widgets::status_badge(ui, stage, widgets::StatusTone::Info);
                }
            });
            ui.label(match report.adapter {
                PreviewAdapter::Dolphin => "This Dolphin GameSettings file has a reviewed shared apply and rollback contract. Confirmation is unavailable until the selected codes are staged in this exact preview.",
                PreviewAdapter::Xenia => "This Xenia patch.toml file has a reviewed shared apply and rollback contract. Confirmation is unavailable until the selected patches are staged in this exact preview.",
                _ => "RetroArch trusted catalogue files have a reviewed shared apply and rollback contract. Confirmation is unavailable until the selected per-game catalogue source is materialized in this exact preview.",
            });
            ui.label(format!(
                "Actionable materialized entries in this page state: {actionable}"
            ));
            ui.label("Confirmation is bound to this exact operation. Replacement permission is separate and never preselected. Cancellation before the write phase changes nothing.");
        });
        if actionable == 0 {
            widgets::banner(
                ui,
                "Nothing ready to install",
                "No selected item can be installed safely from this preview.",
                widgets::StatusTone::Pending,
            );
        }
        match transaction {
            CheatTransactionState::Idle if actionable > 0 && source_materialized => {
                if widgets::action_button(ui, "Review changes", widgets::ActionStyle::Primary, true)
                    .clicked()
                {
                    action = Some(CheatWorkflowAction::ReviewApply);
                }
            }
            CheatTransactionState::Review {
                plan,
                replacement_approved,
                ..
            } => {
                widgets::status_badge(ui, "Review changes", widgets::StatusTone::Warning);
                let replacement_required = plan.entries.iter().any(|entry| {
                    entry.proposed_action
                        == archivefs_core::patch_manager::PreviewProposedAction::Replace
                });
                ui.label(format!(
                    "{} selected file{} will be installed.",
                    plan.entries.len(),
                    if plan.entries.len() == 1 { "" } else { "s" }
                ));
                widgets::technical_details(ui, "shared_transaction_exact_plan", |ui| {
                    widgets::copyable_value(ui, "Plan ID", &plan.plan_id);
                    for entry in &plan.entries {
                        ui.separator();
                        ui.label(format!("Action: {:?}", entry.proposed_action));
                        ui.label(format!("Source: {}", entry.source_path.display));
                        ui.label(format!(
                            "Destination: {}/{}",
                            entry.destination_root.display, entry.destination_relative_path.display
                        ));
                        widgets::copyable_value(
                            ui,
                            "Approved source SHA-256",
                            &entry.source_digest,
                        );
                    }
                });
                if replacement_required {
                    ui.checkbox(
                        replacement_approved,
                        "I separately approve replacement of the exact different file shown above",
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    if widgets::action_button(
                        ui,
                        "Confirm and install",
                        widgets::ActionStyle::Primary,
                        !replacement_required || *replacement_approved,
                    )
                    .clicked()
                    {
                        action = Some(CheatWorkflowAction::ConfirmApply);
                    }
                    if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true)
                        .clicked()
                    {
                        action = Some(CheatWorkflowAction::CancelApply);
                    }
                });
            }
            CheatTransactionState::Applying { .. } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Installing and checking the result…");
                });
            }
            CheatTransactionState::Result { result, .. } => {
                let (label, tone) = match result.journal.status {
                    SharedApplyStatus::Success => {
                        ("Installed and verified", widgets::StatusTone::Success)
                    }
                    SharedApplyStatus::PartialFailure => {
                        ("Some changes failed", widgets::StatusTone::Warning)
                    }
                    SharedApplyStatus::Failed => ("Install failed", widgets::StatusTone::Blocked),
                    SharedApplyStatus::DryRun => ("Dry run", widgets::StatusTone::Info),
                };
                widgets::status_badge(ui, label, tone);
                ui.label(format!(
                    "{} file result{} recorded.",
                    result.journal.entries.len(),
                    if result.journal.entries.len() == 1 {
                        ""
                    } else {
                        "s"
                    }
                ));
                if result.journal_failure.is_some() {
                    widgets::banner(
                        ui,
                        "Undo information could not be saved",
                        "The file operation finished, but EmuWiz could not save all of its recovery information. Check Technical details before making more changes.",
                        widgets::StatusTone::Warning,
                    );
                }
                widgets::technical_details(ui, "shared_transaction_result", |ui| {
                    widgets::copyable_value(ui, "Operation ID", &result.journal.operation_id);
                    for entry in &result.journal.entries {
                        ui.label(format!(
                            "{:?} · verification {} · backup {}",
                            entry.outcome,
                            if entry.verification_succeeded {
                                "passed"
                            } else {
                                "not complete"
                            },
                            if entry.backup_path.is_some() {
                                "retained"
                            } else {
                                "not required"
                            }
                        ));
                        for failure in &entry.failures {
                            ui.label(format!("Failure: {:?} · {}", failure.kind, failure.detail));
                        }
                    }
                    if let Some(failure) = &result.journal_failure {
                        ui.label(format!(
                            "Recovery journal failure: {:?} · {}",
                            failure.kind, failure.detail
                        ));
                    }
                    if let Some(path) = &result.journal_path
                        && widgets::path_value(ui, "Journal", path)
                    {
                        let _ = clipboard.set_text(path.display().to_string());
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    if widgets::action_button(
                        ui,
                        "Open exact operation in History & Logs",
                        widgets::ActionStyle::Secondary,
                        result.journal_path.is_some(),
                    )
                    .clicked()
                    {
                        action = Some(CheatWorkflowAction::OpenApplyHistory);
                    }
                    // Rollback is journal-backed, so it is offered exactly
                    // when a journal exists to roll back from - including
                    // after a partial failure, which is when it matters most.
                    let rollback_available = result.journal_path.is_some()
                        && matches!(
                            result.journal.status,
                            SharedApplyStatus::Success | SharedApplyStatus::PartialFailure
                        );
                    if widgets::action_button(
                        ui,
                        "Roll back this install",
                        widgets::ActionStyle::Destructive,
                        rollback_available,
                    )
                    .clicked()
                    {
                        action = Some(CheatWorkflowAction::RollbackInstall);
                    }
                });
                if result.journal_path.is_none() {
                    ui.label(
                        "No journal was written for this operation, so there is nothing to roll back from.",
                    );
                }
            }
            CheatTransactionState::Idle => {}
        }
    });
    action
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn show_cheats_mods_page(
    ui: &mut egui::Ui,
    workflow: Option<&mut CheatWorkflowState>,
    profiles: &RetroArchProfilesState,
    pcsx2_profiles: &Pcsx2ProfilesState,
    dolphin_profiles: &DolphinProfilesState,
    xenia_profiles: &XeniaProfilesState,
    live: Option<&LoadedData>,
    cached: Option<&CachedLibrarySnapshot>,
    history: &OperationHistory,
    busy: bool,
    clipboard: &mut dyn ClipboardBackend,
    dolphin_texture_mod: &mut crate::dolphin_texture_mod_page::DolphinTextureModPageState,
    local_mod_package: &mut crate::local_mod_package_page::LocalModPackagePageState,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    let activity_archive = workflow
        .as_deref()
        .map(|workflow| workflow.archive_path.clone());
    let pcsx2_read_only = workflow
        .as_deref()
        .is_some_and(|workflow| workflow.adapter == CheatEmulatorAdapter::Pcsx2);
    let dolphin_read_only = workflow
        .as_deref()
        .is_some_and(|workflow| workflow.adapter == CheatEmulatorAdapter::Dolphin);
    // Every selected-game route starts in the gamer-facing presentation.
    // Adapter state and audit evidence remain available in Workflow
    // diagnostics instead of determining whether the page is approachable.
    let beginner_route = workflow.is_some();
    widgets::page_header_with_icon(
        ui,
        crate::ui::icons::CHEATS,
        "Cheats & Mods",
        if beginner_route {
            "Choose compatible enhancements for the selected game."
        } else {
            "Find cheats, patches and game enhancements for a selected game."
        },
    );
    // One restrained retro cheat-code motif - decoration, never the label.
    ui.label(
        egui::RichText::new(crate::ui::icons::CHEAT_CODE)
            .color(theme::muted(ui))
            .size(13.0),
    );
    ui.add_space(theme::SECTION_GAP / 2.0);

    if beginner_route && let Some(workflow) = workflow.as_deref() {
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("Selected game: {}", workflow.display_name));
            widgets::status_badge(
                ui,
                workflow.platform.as_deref().unwrap_or("Unknown platform"),
                widgets::StatusTone::Info,
            );
            if widgets::action_button(ui, "Choose another game", widgets::ActionStyle::Quiet, true)
                .clicked()
            {
                action = Some(CheatWorkflowAction::ChooseArchive);
            }
        });
        ui.add_space(theme::SECTION_GAP);
        crate::local_mod_package_page::show_local_mod_package_panel(
            ui,
            local_mod_package,
            &workflow.archive_path,
            crate::ready_game_identity(workflow),
        );
        ui.add_space(theme::SECTION_GAP);
    }

    // --- Overview: current archive, its readiness, and availability
    // across every supported system - a concise summary, not a deep dive
    // into whichever system happens to be selected right now (that lives
    // in "Selected system workflow" below).
    if !beginner_route {
        widgets::section_header(
            ui,
            "Overview",
            Some(
                "The selected archive, its readiness, and what each supported system can do with it.",
            ),
        );
        let (integration_label, integration_tone) = match workflow.as_deref() {
            Some(workflow) if workflow.adapter == CheatEmulatorAdapter::Pcsx2 => {
                pcsx2_integration_presentation(pcsx2_profiles)
            }
            Some(workflow) if workflow.adapter == CheatEmulatorAdapter::Dolphin => {
                dolphin_integration_presentation(dolphin_profiles)
            }
            Some(workflow) if workflow.adapter == CheatEmulatorAdapter::Unsupported => (
                "Unsupported platform".to_string(),
                widgets::StatusTone::Warning,
            ),
            _ => retroarch_integration_presentation(profiles),
        };
        widgets::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                widgets::status_badge(ui, integration_label, integration_tone);
                if let Some(workflow) = workflow.as_deref() {
                    ui.strong(format!("Selected game: {}", workflow.display_name));
                } else {
                    ui.label("Choose a game");
                }
                if widgets::action_button(
                    ui,
                    if workflow.is_some() {
                        "Choose another archive"
                    } else {
                        "Choose archive"
                    },
                    widgets::ActionStyle::Secondary,
                    true,
                )
                .clicked()
                {
                    action = Some(CheatWorkflowAction::ChooseArchive);
                }
            });
            let mut readiness_items = Vec::new();
            if workflow.is_none() {
                readiness_items.push(("Waiting for a game", widgets::StatusTone::Pending));
            } else if pcsx2_read_only {
                readiness_items.push(("Read-only preview", widgets::StatusTone::Info));
                readiness_items.push(("Preview only", widgets::StatusTone::Pending));
            } else {
                readiness_items.push(("Shared matching available", widgets::StatusTone::Info));
                readiness_items.push((
                    "Controlled apply after eligible preview",
                    widgets::StatusTone::Info,
                ));
            }
            widgets::status_strip(ui, &readiness_items);
        });
        ui.add_space(theme::SECTION_GAP);
    }

    // --- Selected system workflow: everything specific to the archive
    // and the currently selected adapter.
    if !beginner_route {
        widgets::section_header(
            ui,
            "Selected system workflow",
            Some(
                "Profile, source, identity, preview, and installation state for the chosen system.",
            ),
        );
    }
    if let Some(workflow) = workflow {
        if !beginner_route {
            show_cheat_archive_context(ui, workflow, live, cached, clipboard);
            ui.add_space(theme::SECTION_GAP);
        }
        // Step order matters here: profile/source selection first, then
        // the preview that depends on them - showing the preview above an
        // unfilled profile picker used to read as "Preview waiting" before
        // the reader had even seen what it was waiting for. Also, only the
        // RetroArch adapter has any shared preview/install pipeline at all
        // (PCSX2 and Dolphin are read-only-only, per the Mods section) -
        // rendering `show_shared_cheat_preview` unconditionally used to
        // show a permanently-empty "Preview waiting" card even while a
        // read-only adapter was selected.
        match workflow.adapter {
            CheatEmulatorAdapter::RetroArch => {
                action = show_cheat_workflow_step1(ui, workflow, profiles, busy).or(action);
                action = show_cheat_source_modes(ui, workflow, profiles).or(action);
                let source_action = match workflow.source_mode {
                    CheatSourceMode::ExistingRetroArchLibrary => {
                        show_existing_retroarch_library(ui, workflow, profiles, clipboard)
                    }
                    CheatSourceMode::ArchiveFsTrustedCatalogue => {
                        show_cheat_workflow_step2(ui, workflow, busy, clipboard)
                    }
                };
                action = action.or(source_action);
                // Stages 4-6 exist only for the trusted-catalogue install
                // path; the existing-library mode is a read-only inspection
                // with nothing to select or generate.
                if workflow.source_mode == CheatSourceMode::ArchiveFsTrustedCatalogue {
                    ui.add_space(theme::SECTION_GAP);
                    action = show_cheat_candidate_stages(ui, workflow, clipboard).or(action);
                }
                ui.add_space(theme::SECTION_GAP);
                action = show_shared_cheat_preview(ui, workflow, clipboard).or(action);
            }
            CheatEmulatorAdapter::Pcsx2 => {
                action = show_pcsx2_workflow(ui, workflow, pcsx2_profiles, clipboard).or(action);
            }
            CheatEmulatorAdapter::Dolphin => {
                action =
                    show_dolphin_workflow(ui, workflow, dolphin_profiles, clipboard).or(action);
                // Immediately after the existing Dolphin cheat workflow and
                // before Workflow diagnostics - its own separate panel, not
                // a cheat stage (see `dolphin_texture_mod_page`'s own
                // module doc comment for why it is never folded into
                // `CheatWorkflowState`).
                let selected_profile = match dolphin_profiles {
                    DolphinProfilesState::Ready(discovery) => workflow
                        .selected_dolphin_profile_id
                        .as_deref()
                        .and_then(|id| {
                            discovery
                                .profiles
                                .iter()
                                .find(|profile| profile.profile_id == id)
                        }),
                    _ => None,
                };
                ui.add_space(theme::SECTION_GAP);
                match selected_profile {
                    Some(profile) => {
                        let identity_report = crate::ready_game_identity(workflow);
                        crate::dolphin_texture_mod_page::show_dolphin_texture_mod_panel(
                            ui,
                            dolphin_texture_mod,
                            &workflow.archive_path,
                            profile,
                            identity_report,
                        );
                    }
                    None => {
                        widgets::section_header(ui, "Dolphin texture mod", None);
                        widgets::card(ui, |ui| {
                            ui.label("Select a Dolphin profile above first.");
                        });
                    }
                }
            }
            CheatEmulatorAdapter::Xenia => {
                action = show_xenia_workflow(ui, workflow, xenia_profiles, clipboard).or(action);
            }
            CheatEmulatorAdapter::Unsupported => {
                widgets::banner(
                    ui,
                    &match workflow.platform.as_deref() {
                        Some(platform) => format!("{platform} recognised"),
                        None => "Platform not recognised".to_string(),
                    },
                    match workflow.platform.as_deref() {
                        Some(_) => {
                            "This platform is recognised, but cheat support is not available yet. Assign a different platform in Library if this is wrong."
                        }
                        None => {
                            "EmuWiz could not determine this archive's platform, so no Cheats & Mods adapter can be chosen. Assign a platform in Library if you know it."
                        }
                    },
                    widgets::StatusTone::Info,
                );
            }
        }

        // --- Diagnostics: everything a user needs only occasionally
        // (workflow-state badges, bounded identity evidence, safety/
        // privacy copy) lives below the primary flow, collapsed by
        // default, rather than between the archive picker and the first
        // real action - see the Cheats & Mods workflow simplification.
        ui.add_space(theme::SECTION_GAP);
        egui::CollapsingHeader::new("Workflow diagnostics")
            .default_open(false)
            .show(ui, |ui| {
                if beginner_route {
                    show_cheat_archive_context(ui, workflow, live, cached, clipboard);
                    ui.add_space(theme::SECTION_GAP);
                }
                show_cheats_mods_workflow_states(
                    ui,
                    Some(workflow),
                    profiles,
                    pcsx2_profiles,
                    dolphin_profiles,
                );
                ui.add_space(theme::SECTION_GAP);
                show_shared_game_identity(ui, workflow, clipboard);
                ui.add_space(theme::SECTION_GAP);
                show_cheats_mods_safety_information(ui);
                if beginner_route {
                    ui.add_space(theme::SECTION_GAP);
                    show_mods_section(ui, pcsx2_read_only, dolphin_read_only);
                }
            });
    } else {
        widgets::card(ui, |ui| {
            widgets::section_header(
                ui,
                "Choose a game",
                Some("Select a game to see available cheats and patches."),
            );
            ui.horizontal_wrapped(|ui| {
                if widgets::action_button(ui, "Choose a game", widgets::ActionStyle::Primary, true)
                    .clicked()
                {
                    action = Some(CheatWorkflowAction::ChooseArchive);
                }
                if widgets::action_button(ui, "Open Library", widgets::ActionStyle::Secondary, true)
                    .clicked()
                {
                    action = Some(CheatWorkflowAction::OpenLibrary);
                }
            });
        });
    }

    if activity_archive.is_some() {
        ui.add_space(theme::SECTION_GAP);
        show_recent_cheat_activity(ui, history, activity_archive.as_deref());
    }
    action
}
