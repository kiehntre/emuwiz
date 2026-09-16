//! Library View action protocol and pure controller helpers.
//!
//! The page renderer remains in `administration_pages.rs`; this module owns
//! the typed actions, dialog state, worker result, and messages shared by the
//! renderer and `ArchiveFsApp`'s thin coordination methods.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use eframe::egui;

use archivefs_core::{
    FrontendPlatformMapping, FrontendProfile, FrontendProfileKind, LibraryViewApplyReport,
    LibraryViewConfig, LibraryViewLayoutTemplate, LibraryViewPlan, LibraryViewPlanAction,
    add_library_view_default, apply_library_view_default, edit_library_view_default,
    load_library_view_configs_default, preview_library_view_default, remove_library_view_default,
    repair_library_view_default, set_library_view_enabled_default,
};

use crate::activity_history::{ActivityAction, ActivityOutcome, HistoryEntry};
use crate::{ActionFeedback, ArchiveFsApp};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LibraryViewAction {
    Add {
        name: String,
        destination_root: PathBuf,
        source_folders: Vec<PathBuf>,
        platforms: Vec<String>,
        profile: FrontendProfile,
    },
    Edit {
        identifier: String,
        name: String,
        destination_root: PathBuf,
        source_folders: Vec<PathBuf>,
        platforms: Vec<String>,
        profile: FrontendProfile,
    },
    SetEnabled {
        identifier: String,
        enabled: bool,
    },
    Preview(String),
    Apply(String),
    Repair(String),
    Remove {
        identifier: String,
        keep_definition: bool,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum LibraryViewActionOutcome {
    Added(LibraryViewConfig),
    Edited(LibraryViewConfig),
    SetEnabled(LibraryViewConfig),
    Previewed {
        view: LibraryViewConfig,
        plan: LibraryViewPlan,
    },
    Applied {
        view: LibraryViewConfig,
        report: LibraryViewApplyReport,
        skipped: Option<usize>,
    },
    Repaired {
        view: LibraryViewConfig,
        report: LibraryViewApplyReport,
        skipped: Option<usize>,
    },
    Removed {
        view: LibraryViewConfig,
        report: LibraryViewApplyReport,
        kept_definition: bool,
    },
}

pub(crate) struct RunningLibraryViewAction {
    pub(crate) action: LibraryViewAction,
    pub(crate) receiver: Receiver<Result<LibraryViewActionOutcome, String>>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LibraryViewFormDialogState {
    pub(crate) editing_id: Option<String>,
    pub(crate) name: String,
    pub(crate) destination_text: String,
    pub(crate) selected_source_folders: HashSet<PathBuf>,
    pub(crate) selected_platforms: HashSet<String>,
    pub(crate) validation_message: Option<String>,
    pub(crate) profile_kind: FrontendProfileKind,
    pub(crate) romm_overrides: Vec<(String, String)>,
    pub(crate) romm_override_platform_input: String,
    pub(crate) romm_override_slug_input: String,
}

#[derive(Clone, Debug)]
pub(crate) struct LibraryViewRemoveDialogState {
    pub(crate) view_id: String,
    pub(crate) view_name: String,
    pub(crate) keep_definition: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LibraryViewPlanFilter {
    #[default]
    All,
    Create,
    Correct,
    Repair,
    Remove,
    Collision,
    Skip,
}

impl LibraryViewPlanFilter {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Create => "Create",
            Self::Correct => "Correct",
            Self::Repair => "Repair",
            Self::Remove => "Remove",
            Self::Collision => "Collision",
            Self::Skip => "Skip",
        }
    }

    pub(crate) fn matches(self, action: LibraryViewPlanAction) -> bool {
        match self {
            Self::All => action != LibraryViewPlanAction::AlreadyCorrect,
            Self::Create => action == LibraryViewPlanAction::Create,
            Self::Correct => action == LibraryViewPlanAction::AlreadyCorrect,
            Self::Repair => action == LibraryViewPlanAction::Repair,
            Self::Remove => action == LibraryViewPlanAction::RemoveStale,
            Self::Collision => action == LibraryViewPlanAction::Collision,
            Self::Skip => matches!(
                action,
                LibraryViewPlanAction::SkipUnknownPlatform
                    | LibraryViewPlanAction::SkipMissingSourceArchive
                    | LibraryViewPlanAction::SkipInvalidPath
            ),
        }
    }
}

pub(crate) const LIBRARY_VIEW_DIALOG_MAX_WIDTH: f32 = 780.0;
pub(crate) const LIBRARY_VIEW_DIALOG_MAX_HEIGHT: f32 = 720.0;

pub(crate) fn library_view_dialog_size(viewport_size: egui::Vec2) -> egui::Vec2 {
    egui::vec2(
        (viewport_size.x - 24.0).clamp(320.0, LIBRARY_VIEW_DIALOG_MAX_WIDTH),
        (viewport_size.y - 24.0).clamp(360.0, LIBRARY_VIEW_DIALOG_MAX_HEIGHT),
    )
}

pub(crate) fn library_view_selections_side_by_side(dialog_width: f32) -> bool {
    dialog_width >= 680.0
}

pub(crate) fn library_view_submit_blocker(
    name: &str,
    destination: &str,
    busy: bool,
) -> Option<&'static str> {
    if busy {
        Some("Wait for the current Library View operation to finish.")
    } else if name.trim().is_empty() {
        Some("Enter a name for this Library View.")
    } else if destination.trim().is_empty() {
        Some("Choose a destination folder for this Library View.")
    } else {
        None
    }
}

pub(crate) fn library_view_form_profile(dialog: &LibraryViewFormDialogState) -> FrontendProfile {
    let mut platform_mapping_overrides = FrontendPlatformMapping::default();
    for (platform, slug) in &dialog.romm_overrides {
        platform_mapping_overrides.insert(platform.clone(), slug.clone());
    }
    FrontendProfile {
        kind: dialog.profile_kind,
        policy: archivefs_core::FrontendProfilePolicy {
            platform_mapping_overrides,
            ..Default::default()
        },
    }
}

pub(crate) fn library_view_action_log_category(action: &LibraryViewAction) -> ActivityAction {
    match action {
        LibraryViewAction::Add { .. } => ActivityAction::LibraryViewAdded,
        LibraryViewAction::Edit { .. } => ActivityAction::LibraryViewEdited,
        LibraryViewAction::SetEnabled { enabled: true, .. } => ActivityAction::LibraryViewEnabled,
        LibraryViewAction::SetEnabled { enabled: false, .. } => ActivityAction::LibraryViewDisabled,
        LibraryViewAction::Preview(_) => ActivityAction::LibraryViewPreview,
        LibraryViewAction::Apply(_) => ActivityAction::LibraryViewApply,
        LibraryViewAction::Repair(_) => ActivityAction::LibraryViewRepair,
        LibraryViewAction::Remove { .. } => ActivityAction::LibraryViewRemoved,
    }
}

pub(crate) fn library_view_action_started_message(action: &LibraryViewAction) -> String {
    match action {
        LibraryViewAction::Add { name, .. } => format!("Adding library view '{name}'."),
        LibraryViewAction::Edit { name, .. } => format!("Saving changes to library view '{name}'."),
        LibraryViewAction::SetEnabled {
            identifier,
            enabled: true,
        } => format!("Enabling library view '{identifier}'."),
        LibraryViewAction::SetEnabled {
            identifier,
            enabled: false,
        } => format!("Disabling library view '{identifier}'."),
        LibraryViewAction::Preview(identifier) => {
            format!("Previewing library view '{identifier}'.")
        }
        LibraryViewAction::Apply(identifier) => format!("Applying library view '{identifier}'."),
        LibraryViewAction::Repair(identifier) => format!("Repairing library view '{identifier}'."),
        LibraryViewAction::Remove {
            identifier,
            keep_definition: true,
        } => format!(
            "Removing managed symlinks for library view '{identifier}' (keeping its definition)."
        ),
        LibraryViewAction::Remove {
            identifier,
            keep_definition: false,
        } => format!("Removing library view '{identifier}' and its managed symlinks."),
    }
}

pub(crate) fn library_view_apply_summary_message(
    verb: &str,
    view_name: &str,
    report: &LibraryViewApplyReport,
    skipped: Option<usize>,
) -> String {
    let base = format!(
        "{verb} '{}': {} created, {} repaired, {} removed, {} unchanged, {} failed",
        view_name, report.created, report.repaired, report.removed, report.unchanged, report.failed
    );
    match skipped {
        Some(0) => format!("{base}, 0 skipped."),
        Some(skipped) => format!(
            "{base}, {skipped} skipped - this view is not fully applied. Unresolved platform \
             mappings or collisions remain; see Preview for details."
        ),
        None => format!("{base}."),
    }
}

pub(crate) fn library_view_action_success_message(outcome: &LibraryViewActionOutcome) -> String {
    match outcome {
        LibraryViewActionOutcome::Added(view) => format!(
            "Library view added: {} -> {}.",
            view.name,
            view.destination_root.display()
        ),
        LibraryViewActionOutcome::Edited(view) => format!("Library view updated: {}.", view.name),
        LibraryViewActionOutcome::SetEnabled(view) => {
            if view.enabled {
                format!("Library view enabled: {}.", view.name)
            } else {
                format!("Library view disabled: {}.", view.name)
            }
        }
        LibraryViewActionOutcome::Previewed { view, plan } => format!(
            "Preview for '{}': {} to create, {} correct, {} to repair, {} to remove, {} \
             collision(s), {} skipped.",
            view.name,
            plan.counts.create,
            plan.counts.correct,
            plan.counts.repair,
            plan.counts.remove,
            plan.counts.collision,
            plan.counts.skip
        ),
        LibraryViewActionOutcome::Applied {
            view,
            report,
            skipped,
        } => library_view_apply_summary_message("Applied", &view.name, report, *skipped),
        LibraryViewActionOutcome::Repaired {
            view,
            report,
            skipped,
        } => library_view_apply_summary_message("Repaired", &view.name, report, *skipped),
        LibraryViewActionOutcome::Removed {
            view,
            report,
            kept_definition,
        } => {
            if *kept_definition {
                format!(
                    "Removed {} managed symlink(s) for '{}'. Its definition was kept.",
                    report.removed, view.name
                )
            } else {
                format!(
                    "Removed {} managed symlink(s) for '{}' and its definition.",
                    report.removed, view.name
                )
            }
        }
    }
}

pub(crate) fn library_view_current_skip_count(view_id: &str) -> Option<usize> {
    preview_library_view_default(view_id)
        .ok()
        .map(|(_, plan)| plan.counts.skip)
}

pub(crate) fn run_library_view_action(
    action: &LibraryViewAction,
) -> Result<LibraryViewActionOutcome, String> {
    match action {
        LibraryViewAction::Add {
            name,
            destination_root,
            source_folders,
            platforms,
            profile,
        } => add_library_view_default(
            name.clone(),
            destination_root.clone(),
            source_folders.clone(),
            platforms.clone(),
            LibraryViewLayoutTemplate::PlatformFilename,
            profile.clone(),
        )
        .map(LibraryViewActionOutcome::Added)
        .map_err(|error| error.to_string()),
        LibraryViewAction::Edit {
            identifier,
            name,
            destination_root,
            source_folders,
            platforms,
            profile,
        } => edit_library_view_default(
            identifier,
            name.clone(),
            destination_root.clone(),
            source_folders.clone(),
            platforms.clone(),
            profile.clone(),
        )
        .map(LibraryViewActionOutcome::Edited)
        .map_err(|error| error.to_string()),
        LibraryViewAction::SetEnabled {
            identifier,
            enabled,
        } => set_library_view_enabled_default(identifier, *enabled)
            .map(LibraryViewActionOutcome::SetEnabled)
            .map_err(|error| error.to_string()),
        LibraryViewAction::Preview(identifier) => preview_library_view_default(identifier)
            .map(|(view, plan)| LibraryViewActionOutcome::Previewed { view, plan })
            .map_err(|error| error.to_string()),
        LibraryViewAction::Apply(identifier) => apply_library_view_default(identifier)
            .map(|(view, report)| {
                let skipped = library_view_current_skip_count(&view.id);
                LibraryViewActionOutcome::Applied {
                    view,
                    report,
                    skipped,
                }
            })
            .map_err(|error| error.to_string()),
        LibraryViewAction::Repair(identifier) => repair_library_view_default(identifier)
            .map(|(view, report)| {
                let skipped = library_view_current_skip_count(&view.id);
                LibraryViewActionOutcome::Repaired {
                    view,
                    report,
                    skipped,
                }
            })
            .map_err(|error| error.to_string()),
        LibraryViewAction::Remove {
            identifier,
            keep_definition,
        } => remove_library_view_default(identifier, *keep_definition)
            .map(|(view, report)| LibraryViewActionOutcome::Removed {
                view,
                report,
                kept_definition: *keep_definition,
            })
            .map_err(|error| error.to_string()),
    }
}

pub(crate) fn load_library_views() -> Vec<LibraryViewConfig> {
    load_library_view_configs_default().unwrap_or_default()
}

pub(crate) fn start_library_view_worker(
    action: LibraryViewAction,
    context: egui::Context,
) -> RunningLibraryViewAction {
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker_action = action.clone();
    std::thread::spawn(move || {
        let result = run_library_view_action(&worker_action);
        let _ = sender.send(result);
        context.request_repaint();
    });
    RunningLibraryViewAction { action, receiver }
}

impl ArchiveFsApp {
    /// Whether a new Library Views action may start - the same "one
    /// writer at a time" convention `source_action_available` uses
    /// (Preview/Apply reads the same database a scan writes to).
    pub(crate) fn library_view_action_available(&self) -> bool {
        self.library_view_action.is_none()
            && self.sources_ui.source_action.is_none()
            && self.library_ui.alias_action.is_none()
            && self.library_ui.platform_action.is_none()
            && self.library_ui.bulk_platform_action.is_none()
            && self.library_ui.missing_removal.is_none()
            && !self.database_state.is_loading()
    }

    /// Re-reads the configured Library Views list from disk - a cheap,
    /// synchronous, read-only file read (the same kind
    /// `source_action_log_category`'s doc comment already calls out for
    /// config reads of this size), never a background thread. A read
    /// failure is treated as "no views configured" here rather than
    /// surfaced as an error, matching `load_library_view_configs_default`'s
    /// own "missing file is not an error" contract - the only way this can
    /// actually fail once the file exists is a corrupt/foreign-format
    /// file, which the next successful Add/Edit save overwrites anyway.
    pub(crate) fn reload_library_views(&mut self) {
        self.library_views = load_library_views();
    }

    pub(crate) fn start_library_view_action(
        &mut self,
        context: egui::Context,
        action: LibraryViewAction,
    ) {
        if !self.library_view_action_available() {
            return;
        }
        self.history.record(HistoryEntry::new(
            library_view_action_log_category(&action),
            None,
            ActivityOutcome::Started,
            library_view_action_started_message(&action),
        ));
        self.library_view_action = Some(start_library_view_worker(action, context));
    }

    /// Mirrors `poll_source_action`: refreshes `library_views` from disk on
    /// every successful outcome (Add/Edit/SetEnabled/Remove all mutate the
    /// configured list; Preview/Apply/Repair don't, but reloading anyway is
    /// harmless and keeps this one code path for all six), and updates
    /// `library_view_last_plan` for the two outcomes that produce or
    /// invalidate a plan.
    pub(crate) fn poll_library_view_action(&mut self, context: &egui::Context) {
        let result = self.library_view_action.as_ref().and_then(|running| {
            running
                .receiver
                .try_recv()
                .ok()
                .map(|result| (running.action.clone(), result))
        });
        let Some((action, result)) = result else {
            return;
        };
        self.library_view_action = None;
        let log_category = library_view_action_log_category(&action);
        match result {
            Ok(outcome) => {
                let message = library_view_action_success_message(&outcome);
                self.history.record(HistoryEntry::new(
                    log_category,
                    None,
                    ActivityOutcome::Completed,
                    message.clone(),
                ));
                self.feedback = Some(ActionFeedback {
                    succeeded: true,
                    message,
                    cleanup: None,
                    warning: None,
                    more_information: None,
                });
                match &outcome {
                    LibraryViewActionOutcome::Added(_) => {
                        self.library_view_form_dialog = None;
                    }
                    LibraryViewActionOutcome::Edited(view) => {
                        self.library_view_form_dialog = None;
                        // The edited view's own filters may have changed -
                        // any previously computed plan for it no longer
                        // reflects the current configuration truthfully.
                        if self
                            .library_view_last_plan
                            .as_ref()
                            .is_some_and(|(previous, _)| previous.id == view.id)
                        {
                            self.library_view_last_plan = None;
                        }
                    }
                    LibraryViewActionOutcome::Previewed { view, plan } => {
                        self.library_view_last_plan = Some((view.clone(), plan.clone()));
                    }
                    LibraryViewActionOutcome::Applied { view, .. }
                    | LibraryViewActionOutcome::Repaired { view, .. } => {
                        // Apply/Repair change what is actually on disk -
                        // the previous plan's Create/Repair/Remove counts
                        // no longer describe reality, even though the
                        // view itself is unchanged.
                        if self
                            .library_view_last_plan
                            .as_ref()
                            .is_some_and(|(previous, _)| previous.id == view.id)
                        {
                            self.library_view_last_plan = None;
                        }
                    }
                    LibraryViewActionOutcome::Removed { view, .. } => {
                        self.library_view_remove_dialog = None;
                        if self
                            .library_view_last_plan
                            .as_ref()
                            .is_some_and(|(previous, _)| previous.id == view.id)
                        {
                            self.library_view_last_plan = None;
                        }
                    }
                    LibraryViewActionOutcome::SetEnabled(_) => {}
                }
                self.reload_library_views();
            }
            Err(message) => {
                self.history.record(HistoryEntry::new(
                    log_category,
                    None,
                    ActivityOutcome::Failed,
                    message.clone(),
                ));
                self.feedback = Some(ActionFeedback {
                    succeeded: false,
                    message,
                    cleanup: None,
                    warning: None,
                    more_information: None,
                });
                // Deliberately does not clear `library_view_form_dialog`/
                // `library_view_remove_dialog` on failure - exactly like
                // `poll_source_action`, a failed Add/Edit/Remove should
                // leave the dialog open with its input intact.
            }
        }
        context.request_repaint();
    }
}
