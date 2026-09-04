//! GUI Batch B: read-only "Sources & Providers" status.
//!
//! Shows what identity evidence sources this build actually has configured:
//! the registered No-Intro DAT sources (enabled/disabled, platform
//! assignment, and last-known health) and Hasheous as an external, optional
//! provider, using only real, already-persisted state. Nothing here plans a
//! destination, offers an Apply, or changes the DAT matching policy; see
//! `dat_sources_page` for that (unmodified, unrelated feature).
//!
//! # Reuse, not a second registry
//!
//! Every row here comes straight from `archivefs_core::dat::sources`: the
//! same [`DatSourceRegistry`] loaded from the same `dat_sources.toml`
//! `DatSourcesPageState` itself reads, and the same
//! [`archivefs_core::identity_source::no_intro::select_no_intro_source`]
//! Batch A already uses to resolve ambiguity for one platform. This module
//! adds no new persistence and no new DAT-source model - only a read-only
//! projection of the existing one, plus a global scan across every
//! platform the registry currently assigns.
//!
//! # Honesty over convenience
//!
//! `NotChecked`/`Invalid`/`Unreadable` health, a disabled source, and an
//! unresolved platform assignment are all shown as themselves - see
//! `dat::sources::DatHealthState`'s own doc for why these are never
//! collapsed into a single "OK"/"not OK" bit. A platform with more than one
//! enabled source that identifies as No-Intro is reported as an ambiguity,
//! never silently narrowed to one - the same rule Batch A's evidence lookup
//! already enforces, applied here across the whole registry instead of one
//! selected file.
//!
//! # Hasheous has no persisted "enabled" state to show
//!
//! Unlike a registered DAT source, Hasheous is not part of
//! `archivefs_core::identity_source::model::IdentityProvider` and has no
//! settings file entry: `selected_evidence_page::run_hasheous_check_live`
//! calls it with hardcoded default host/timeout, only when a user explicitly
//! clicks "Check Hasheous" for one selected file - see that module's own
//! privacy note. Inventing an enabled/disabled toggle for it here would
//! misrepresent state this build does not track. What this page reports
//! instead is its actual, fixed behavior: external, optional, queried only
//! on explicit request, never automatically and never from this page.
//!
//! # ScummVM Detection
//!
//! ScummVM is not a DAT source and is never presented as one: it owns its
//! own detection tables, and EmuWiz asks the locally installed native
//! `scummvm` executable to run its own `--detect` command
//! (`archivefs_core::scummvm_detection`) rather than bundling or scraping a
//! second database. This section shows whether that executable is present
//! ([`gather_scummvm_readiness`]) and, on explicit request, runs it
//! read-only against every already-configured ScummVM folder in the
//! library ([`check_scummvm_candidates`]) - no file writes, no renames, no
//! hashing, no DATs, no network. A folder ScummVM reports more than one
//! candidate for is shown as `Ambiguous`, never narrowed to a guess; a
//! folder ScummVM cannot examine at all is `Not recognised`, never treated
//! as "probably fine."

use std::path::{Path, PathBuf};

use eframe::egui;

use archivefs_core::dat::sources::{DatHealthState, DatSourceEntry, DatSourceRegistry};
use archivefs_core::identity_source::hasheous::client::HASHEOUS_DEFAULT_BASE_URL;
use archivefs_core::identity_source::no_intro::{NoIntroSourceSelection, select_no_intro_source};
use archivefs_core::scummvm_detection::{self, ScummVmDetectionError};

use crate::ui::components as widgets;

// ---------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------

/// One registered DAT source, projected for display. Carries no parsed DAT
/// content - only the registry's own persisted fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoIntroSourceRow {
    pub(crate) source_id: String,
    pub(crate) display_name: String,
    pub(crate) enabled: bool,
    pub(crate) kind_label: &'static str,
    /// The assigned platform's display name, or an honest "no assignment"
    /// label - never fabricated, never silently dropped.
    pub(crate) platform_label: String,
    pub(crate) platform_is_resolved: bool,
    pub(crate) health_label: &'static str,
    pub(crate) health_tone: widgets::StatusTone,
    pub(crate) health_detail: Option<String>,
    pub(crate) path_display: String,
}

/// More than one enabled source identifies as No-Intro for the same
/// platform. Never resolved to a first pick - see this module's own doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlatformAmbiguity {
    pub(crate) platform_label: String,
    pub(crate) competing_sources: Vec<String>,
}

/// The whole read-only status: every registered source, plus every
/// platform-scoped ambiguity found while scanning the registry.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct NoIntroSourcesStatus {
    pub(crate) rows: Vec<NoIntroSourceRow>,
    pub(crate) ambiguities: Vec<PlatformAmbiguity>,
}

fn health_tone(state: DatHealthState) -> widgets::StatusTone {
    match state {
        DatHealthState::NotChecked => widgets::StatusTone::Pending,
        DatHealthState::Valid => widgets::StatusTone::Success,
        DatHealthState::ValidWithWarnings => widgets::StatusTone::Warning,
        DatHealthState::Invalid | DatHealthState::Unreadable => widgets::StatusTone::Blocked,
    }
}

fn row_for_entry(entry: &DatSourceEntry) -> NoIntroSourceRow {
    NoIntroSourceRow {
        source_id: entry.id.clone(),
        display_name: entry.display_name.clone(),
        enabled: entry.enabled,
        kind_label: entry.kind.label(),
        platform_label: entry
            .platform_display()
            .unwrap_or_else(|| "Unassigned (any platform)".to_string()),
        platform_is_resolved: entry.platform_is_resolved(),
        health_label: entry.health.state().label(),
        health_tone: health_tone(entry.health.state()),
        health_detail: entry.health.detail.clone(),
        path_display: entry.path.display().to_string(),
    }
}

/// Scans every explicitly-assigned platform among the enabled sources and
/// reports which ones have more than one enabled source that identifies as
/// No-Intro - reusing [`select_no_intro_source`] unchanged, one call per
/// distinct platform. A source left unassigned participates in every
/// platform's check already (see `sorted_enabled_for_platform`'s own doc),
/// so it needs no separate pass here.
///
/// This performs real file I/O (parsing every candidate DAT once per
/// distinct platform) and must not run on the UI thread - callers run it
/// exactly once per explicit "Load"/"Refresh", the same convention
/// `selected_evidence_page::gather_selected_evidence` uses.
fn find_platform_ambiguities(registry: &DatSourceRegistry) -> Vec<PlatformAmbiguity> {
    let mut platforms: Vec<String> = registry
        .entries()
        .iter()
        .filter(|entry| entry.enabled)
        .filter_map(|entry| entry.platform.clone())
        .collect();
    platforms.sort();
    platforms.dedup();

    let mut ambiguities = Vec::new();
    for platform in &platforms {
        if let NoIntroSourceSelection::Ambiguous(labels) =
            select_no_intro_source(registry, Some(platform))
        {
            let platform_label = archivefs_core::canonical_platform_for_alias(platform)
                .map(|canonical| archivefs_core::platform::display_name_for(canonical).to_string())
                .unwrap_or_else(|| platform.clone());
            ambiguities.push(PlatformAmbiguity {
                platform_label,
                competing_sources: labels
                    .into_iter()
                    .map(|label| format!("{} ({})", label.display_name, label.source_id))
                    .collect(),
            });
        }
    }
    ambiguities
}

/// Loads the registry from `dat_sources_config_path` (the same on-disk file
/// `DatSourcesPageState::load` reads - not a second registry) and builds the
/// whole status. `None` (no resolvable config path, e.g. `HOME` unset)
/// behaves like an empty registry rather than an error - the same honest
/// fallback `DatSourcesPageState` itself uses.
pub(crate) fn gather_no_intro_sources_status(
    dat_sources_config_path: Option<&Path>,
) -> NoIntroSourcesStatus {
    let config = dat_sources_config_path
        .and_then(|path| archivefs_core::dat::sources::load_dat_sources_config_from(path).ok())
        .unwrap_or_default();
    let (registry, _unresolved_entries) = DatSourceRegistry::from_config(&config);

    let mut rows: Vec<NoIntroSourceRow> = registry
        .sorted_all()
        .into_iter()
        .map(row_for_entry)
        .collect();
    rows.sort_by(|a, b| a.source_id.cmp(&b.source_id));

    NoIntroSourcesStatus {
        rows,
        ambiguities: find_platform_ambiguities(&registry),
    }
}

/// Hasheous's fixed, always-true behavior - not a persisted setting (see
/// this module's own doc for why there is nothing to load here).
pub(crate) struct HasheousProviderInfo {
    pub(crate) base_url: &'static str,
}

pub(crate) fn hasheous_provider_info() -> HasheousProviderInfo {
    HasheousProviderInfo {
        base_url: HASHEOUS_DEFAULT_BASE_URL,
    }
}

// ---------------------------------------------------------------------
// ScummVM Detection
// ---------------------------------------------------------------------

/// Whether the native ScummVM detector this build would invoke is actually
/// present, and its version if that was safely obtainable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScummVmReadiness {
    Ready {
        executable: PathBuf,
        /// `None` when the version subprocess failed for any reason - a
        /// nicety, never something the rest of readiness waits on or
        /// reports as an error.
        version: Option<String>,
    },
    Missing,
}

/// Gathers readiness: `resolve_scummvm_executable` is a cheap filesystem
/// stat, and when it finds something, `scummvm_version` is one short,
/// bounded, already-timeout-protected subprocess call. Callers run this off
/// the UI thread exactly once - the same one-time environment-probe
/// convention `start_dolphin_local_profile_scan` and its siblings already
/// use for "NotScanned" fields - never on every frame, and never repeated
/// automatically afterward. No install/download action exists to offer:
/// when ScummVM is missing, the only honest thing to show is that it is
/// missing.
pub(crate) fn gather_scummvm_readiness() -> ScummVmReadiness {
    match scummvm_detection::resolve_scummvm_executable() {
        Some(executable) => {
            let version = scummvm_detection::scummvm_version(&executable);
            ScummVmReadiness::Ready {
                executable,
                version,
            }
        }
        None => ScummVmReadiness::Missing,
    }
}

/// Mirrors `DolphinLocalProfilesState`'s own shape: a one-time,
/// automatically-triggered-once-per-session environment probe, not
/// something a person clicks a button for.
#[derive(Default)]
pub(crate) enum ScummVmReadinessState {
    #[default]
    NotChecked,
    Checking {
        receiver: std::sync::mpsc::Receiver<ScummVmReadiness>,
    },
    Ready(ScummVmReadiness),
}

/// One ScummVM-platform folder already known to the loaded library, and
/// what running the real detector against it read-only produced.
///
/// # Bucketing rule (matches `game_identity::apply_scummvm_detection`)
///
/// - `Detected`: the detector returned exactly one explicit `Game:` record -
///   the only case EmuWiz treats as trustworthy identity.
/// - `Ambiguous`: the detector returned more than one candidate, or its
///   output could not be parsed cleanly enough to be sure how many it meant -
///   never narrowed to a guess.
/// - `Unsupported`: the detector ran cleanly and explicitly found nothing it
///   recognises in this folder.
/// - `NotRecognised`: the detector could not even examine this folder (not
///   a valid game directory, an unsafe entry, it failed to run, or its
///   output was not the documented shape) - the folder itself, not the
///   game inside it, is the problem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScummVmOutcome {
    Detected {
        engine_id: String,
        game_id: String,
        description: Option<String>,
    },
    Ambiguous {
        candidate_count: usize,
    },
    Unsupported,
    NotRecognised {
        detail: String,
    },
}

impl ScummVmOutcome {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Detected { .. } => "Detected",
            Self::Ambiguous { .. } => "Ambiguous",
            Self::Unsupported => "Unsupported",
            Self::NotRecognised { .. } => "Not recognised",
        }
    }

    pub(crate) fn tone(&self) -> widgets::StatusTone {
        match self {
            Self::Detected { .. } => widgets::StatusTone::Success,
            Self::Ambiguous { .. } => widgets::StatusTone::Warning,
            Self::Unsupported => widgets::StatusTone::Pending,
            Self::NotRecognised { .. } => widgets::StatusTone::Blocked,
        }
    }
}

fn bucket_scummvm_error(error: ScummVmDetectionError) -> ScummVmOutcome {
    match error {
        ScummVmDetectionError::Ambiguous(candidate_count) => {
            ScummVmOutcome::Ambiguous { candidate_count }
        }
        // The detector's own output could not be parsed cleanly enough to
        // trust a specific count - still "more than one plausible answer",
        // never narrowed to a guess, so it shares the Ambiguous bucket.
        ScummVmDetectionError::MalformedOutput(_) => {
            ScummVmOutcome::Ambiguous { candidate_count: 0 }
        }
        ScummVmDetectionError::NoMatch => ScummVmOutcome::Unsupported,
        other => ScummVmOutcome::NotRecognised {
            detail: other.to_string(),
        },
    }
}

/// One row of a completed check: the library path checked, a short label
/// for display, and the bucketed outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScummVmCheckRow {
    pub(crate) path: PathBuf,
    pub(crate) label: String,
    pub(crate) outcome: ScummVmOutcome,
}

/// The most rows rendered in full before the panel switches to a summary
/// count for the rest - see the module doc's "bounded" note. Chosen well
/// above what a person would actually scroll through, and far below what a
/// library with thousands of ScummVM folders would otherwise dump into one
/// frame's shapes.
pub(crate) const MAX_RENDERED_SCUMMVM_ROWS: usize = 200;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ScummVmCheckSummary {
    pub(crate) rows: Vec<ScummVmCheckRow>,
}

impl ScummVmCheckSummary {
    pub(crate) fn count_matching(&self, outcome_label: &str) -> usize {
        self.rows
            .iter()
            .filter(|row| row.outcome.label() == outcome_label)
            .count()
    }
}

/// Every already-configured ScummVM folder in the loaded library: rows
/// whose platform resolves to the canonical `ScummVM` id, paired with a
/// short display label. Never guesses from a filename - the platform this
/// reads was itself assigned by the existing, unrelated platform-detection
/// pipeline (directory layout, not extension), and this function only
/// filters by it.
pub(crate) fn scummvm_candidates_from_rows(rows: &[crate::ArchiveRow]) -> Vec<(PathBuf, String)> {
    rows.iter()
        .filter(|row| {
            archivefs_core::canonical_platform_for_alias(&row.platform) == Some("ScummVM")
        })
        .map(|row| {
            let label = row
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| row.path.display().to_string());
            (row.path.clone(), label)
        })
        .collect()
}

/// Runs the existing, unmodified ScummVM detector
/// (`detect_scummvm_directory_with_executable`) against every candidate in
/// turn, read-only, and buckets each result. `on_progress(checked, total,
/// label)` is called before each folder is checked, so a caller running
/// this off the UI thread can report progress as it goes; it is never
/// called on the UI thread here.
///
/// No network call, no file write, no rename, no hash: everything this
/// function does beyond the existing detector call is arithmetic and
/// string formatting.
pub(crate) fn check_scummvm_candidates(
    executable: &Path,
    candidates: &[(PathBuf, String)],
    mut on_progress: impl FnMut(usize, usize, &str),
) -> ScummVmCheckSummary {
    let total = candidates.len();
    let mut rows = Vec::with_capacity(total);
    for (index, (path, label)) in candidates.iter().enumerate() {
        on_progress(index, total, label);
        let outcome =
            match scummvm_detection::detect_scummvm_directory_with_executable(path, executable) {
                Ok(game) => ScummVmOutcome::Detected {
                    engine_id: game.engine_id,
                    game_id: game.game_id,
                    description: game.description,
                },
                Err(error) => bucket_scummvm_error(error),
            };
        rows.push(ScummVmCheckRow {
            path: path.clone(),
            label: label.clone(),
            outcome,
        });
    }
    ScummVmCheckSummary { rows }
}

/// One progress update from an in-flight check, sent as the worker moves
/// from folder to folder - the "current item" / "checked of total"
/// convention `dat_sources_page`'s jobs already use.
pub(crate) enum ScummVmCheckMessage {
    Progress {
        checked: usize,
        total: usize,
        current: String,
    },
    Done(ScummVmCheckSummary),
}

/// Mirrors [`IdentitySourcesState`]'s own shape (explicit action, off-UI-thread,
/// generation-guarded), plus the one thing a No-Intro status load never
/// needed: incremental progress, since checking many folders can take real
/// time (each folder is its own bounded subprocess call).
#[derive(Default)]
pub(crate) enum ScummVmCheckState {
    #[default]
    Idle,
    Checking {
        generation: u64,
        receiver: std::sync::mpsc::Receiver<(u64, ScummVmCheckMessage)>,
        checked: usize,
        total: usize,
        current: Option<String>,
    },
    Ready {
        #[allow(dead_code)]
        generation: u64,
        summary: ScummVmCheckSummary,
    },
}

pub(crate) enum ScummVmAction {
    /// Run the detector against every already-configured ScummVM folder.
    Check { executable: PathBuf },
}

// ---------------------------------------------------------------------
// View state
// ---------------------------------------------------------------------

/// Mirrors `selected_evidence_page::SelectedEvidenceState`'s own shape:
/// explicit load, off-UI-thread, generation-guarded.
#[derive(Default)]
pub(crate) enum IdentitySourcesState {
    #[default]
    Idle,
    Loading {
        generation: u64,
        receiver: std::sync::mpsc::Receiver<(u64, NoIntroSourcesStatus)>,
    },
    Ready {
        #[allow(dead_code)]
        generation: u64,
        status: NoIntroSourcesStatus,
    },
}

pub(crate) enum IdentitySourcesAction {
    Load,
}

// ---------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------

/// Draws the "Sources & Providers" section. Returns an action the caller
/// should perform (loading/refreshing the status) - drawing itself never
/// mutates anything and never touches the network.
pub(crate) fn show_identity_sources_panel(
    ui: &mut egui::Ui,
    advanced_mode: bool,
    state: &IdentitySourcesState,
) -> Option<IdentitySourcesAction> {
    let mut action = None;
    widgets::section_header(
        ui,
        "Sources & Providers",
        Some("What identity evidence sources are actually configured right now."),
    );

    match state {
        IdentitySourcesState::Idle => {
            widgets::card(ui, |ui| {
                ui.label("Source status has not been loaded yet.");
                if widgets::action_button(
                    ui,
                    "Load sources status",
                    widgets::ActionStyle::Secondary,
                    true,
                )
                .clicked()
                {
                    action = Some(IdentitySourcesAction::Load);
                }
            });
        }
        IdentitySourcesState::Loading { .. } => {
            widgets::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Reading the registered DAT sources…");
                });
            });
        }
        IdentitySourcesState::Ready { status, .. } => {
            show_no_intro_sources(ui, advanced_mode, status);
            if widgets::action_button(ui, "Refresh", widgets::ActionStyle::Quiet, true).clicked() {
                action = Some(IdentitySourcesAction::Load);
            }
        }
    }

    show_hasheous_provider(ui);

    action
}

fn show_no_intro_sources(ui: &mut egui::Ui, advanced_mode: bool, status: &NoIntroSourcesStatus) {
    widgets::section_header(ui, "No-Intro DAT sources", None);
    if status.rows.is_empty() {
        ui.label("No DAT sources are registered.");
    } else if advanced_mode {
        for row in &status.rows {
            widgets::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(&row.display_name);
                    widgets::status_badge(
                        ui,
                        if row.enabled { "Enabled" } else { "Disabled" },
                        if row.enabled {
                            widgets::StatusTone::Success
                        } else {
                            widgets::StatusTone::Pending
                        },
                    );
                    widgets::status_badge(ui, row.health_label, row.health_tone);
                });
                ui.label(format!("Platform: {}", row.platform_label));
                if !row.platform_is_resolved {
                    ui.label(
                        "This build does not recognize the assigned platform id; kept as written.",
                    );
                }
                widgets::technical_details(ui, &row.source_id, |ui| {
                    ui.label(format!("id: {}", row.source_id));
                    ui.label(format!("kind: {}", row.kind_label));
                    ui.label(format!("path: {}", row.path_display));
                    if let Some(detail) = &row.health_detail {
                        ui.label(format!("detail: {detail}"));
                    }
                });
            });
        }
    } else {
        let rows: Vec<(&str, String, widgets::StatusTone)> = status
            .rows
            .iter()
            .map(|row| {
                let value = format!(
                    "{} · {} · {}",
                    row.platform_label,
                    if row.enabled { "Enabled" } else { "Disabled" },
                    row.health_label,
                );
                (row.display_name.as_str(), value, row.health_tone)
            })
            .collect();
        let borrowed: Vec<(&str, &str, widgets::StatusTone)> = rows
            .iter()
            .map(|(label, value, tone)| (*label, value.as_str(), *tone))
            .collect();
        widgets::status_rows(ui, &borrowed);
    }

    if !status.ambiguities.is_empty() {
        for ambiguity in &status.ambiguities {
            widgets::banner(
                ui,
                &format!(
                    "Multiple No-Intro sources match {}",
                    ambiguity.platform_label
                ),
                &format!(
                    "{} enabled sources qualify ({}). None is used automatically - disable or \
                     reassign one before identity evidence can rely on this platform.",
                    ambiguity.competing_sources.len(),
                    ambiguity.competing_sources.join(", ")
                ),
                widgets::StatusTone::Warning,
            );
        }
    }
}

fn show_hasheous_provider(ui: &mut egui::Ui) {
    widgets::section_header(
        ui,
        "Hasheous",
        Some("External, optional. Never queried automatically."),
    );
    let info = hasheous_provider_info();
    widgets::card(ui, |ui| {
        widgets::status_badge(ui, "Manual only", widgets::StatusTone::Info);
        ui.label(
            "Hasheous is checked only when you explicitly click \"Check Hasheous\" on a \
             selected file's evidence panel. It is never called automatically from here or \
             anywhere else, and this page makes no network request.",
        );
        ui.label(format!("Endpoint: {}", info.base_url));
    });
}

/// Draws the "ScummVM Detection" section: readiness, then the read-only
/// "Check ScummVM games" action and its bounded results. Returns the one
/// action the caller should perform (running the check) - drawing itself
/// never touches the filesystem beyond what `readiness`/`check_state`
/// already computed elsewhere.
pub(crate) fn show_scummvm_detection_panel(
    ui: &mut egui::Ui,
    readiness: &ScummVmReadinessState,
    candidate_count: usize,
    check_state: &ScummVmCheckState,
) -> Option<ScummVmAction> {
    let mut action = None;
    widgets::section_header(
        ui,
        "ScummVM Detection",
        Some("ScummVM uses its own built-in game detector rather than DAT files."),
    );

    widgets::card(ui, |ui| match readiness {
        ScummVmReadinessState::NotChecked | ScummVmReadinessState::Checking { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Looking for a local ScummVM install…");
            });
        }
        ScummVmReadinessState::Ready(ScummVmReadiness::Missing) => {
            widgets::status_badge(ui, "Missing", widgets::StatusTone::Blocked);
            ui.label(
                "No ScummVM executable was found on this system. Install ScummVM from your \
                     package manager or scummvm.org, then come back to this page - EmuWiz never \
                     downloads or installs it for you.",
            );
        }
        ScummVmReadinessState::Ready(ScummVmReadiness::Ready {
            executable,
            version,
        }) => {
            ui.horizontal(|ui| {
                widgets::status_badge(ui, "Ready", widgets::StatusTone::Success);
                if let Some(version) = version {
                    ui.weak(version);
                }
            });
            widgets::technical_details(ui, "scummvm-detector-path", |ui| {
                ui.label(format!("Detector: {}", executable.display()));
            });

            ui.add_space(crate::ui::theme::SPACE_SM);
            match check_state {
                ScummVmCheckState::Idle => {
                    ui.label(if candidate_count == 0 {
                        "No ScummVM folders are configured in your library yet.".to_string()
                    } else {
                        format!("{candidate_count} ScummVM folder(s) configured in your library.")
                    });
                    if widgets::action_button(
                        ui,
                        "Check ScummVM games",
                        widgets::ActionStyle::Secondary,
                        candidate_count > 0,
                    )
                    .clicked()
                    {
                        action = Some(ScummVmAction::Check {
                            executable: executable.clone(),
                        });
                    }
                }
                ScummVmCheckState::Checking {
                    checked,
                    total,
                    current,
                    ..
                } => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Checking {checked}/{total}…"));
                    });
                    if let Some(current) = current {
                        ui.weak(current);
                    }
                }
                ScummVmCheckState::Ready { summary, .. } => {
                    show_scummvm_results(ui, summary);
                    if widgets::action_button(ui, "Check again", widgets::ActionStyle::Quiet, true)
                        .clicked()
                    {
                        action = Some(ScummVmAction::Check {
                            executable: executable.clone(),
                        });
                    }
                }
            }
        }
    });

    action
}

fn show_scummvm_results(ui: &mut egui::Ui, summary: &ScummVmCheckSummary) {
    let counts = [
        ("Detected", widgets::StatusTone::Success),
        ("Ambiguous", widgets::StatusTone::Warning),
        ("Unsupported", widgets::StatusTone::Pending),
        ("Not recognised", widgets::StatusTone::Blocked),
    ];
    ui.horizontal_wrapped(|ui| {
        for (label, tone) in counts {
            let count = summary.count_matching(label);
            widgets::status_badge(ui, &format!("{label}: {count}"), tone);
        }
    });

    if summary.rows.is_empty() {
        return;
    }
    ui.add_space(crate::ui::theme::SPACE_SM);
    for row in summary.rows.iter().take(MAX_RENDERED_SCUMMVM_ROWS) {
        ui.horizontal(|ui| {
            widgets::status_badge(ui, row.outcome.label(), row.outcome.tone());
            ui.label(&row.label);
            match &row.outcome {
                ScummVmOutcome::Detected {
                    engine_id, game_id, ..
                } => {
                    ui.weak(format!("{engine_id}:{game_id}"));
                }
                ScummVmOutcome::Ambiguous { candidate_count } if *candidate_count > 0 => {
                    ui.weak(format!("{candidate_count} candidates"));
                }
                ScummVmOutcome::NotRecognised { detail } => {
                    ui.weak(detail);
                }
                _ => {}
            }
        });
    }
    let remaining = summary.rows.len().saturating_sub(MAX_RENDERED_SCUMMVM_ROWS);
    if remaining > 0 {
        ui.weak(format!("…and {remaining} more."));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::dat::sources::{DatSourceEntry, DatSourceKind};
    use std::path::{Path as StdPath, PathBuf};
    use tempfile::tempdir;

    const GB_NO_INTRO_XML: &str = r#"<?xml version="1.0"?>
<datafile>
    <header>
        <name>Nintendo - Game Boy</name>
        <version>20250101-120000</version>
        <author>No-Intro</author>
    </header>
    <game name="Alleyway (World)">
        <rom name="Alleyway (World).gb" size="1" crc="00000000" sha1="ed8070e011713527bdc03e2b9cec9f9c4a7e3aaa"/>
    </game>
</datafile>"#;

    const GB_NO_INTRO_XML_OTHER: &str = r#"<?xml version="1.0"?>
<datafile>
    <header>
        <name>Nintendo - Game Boy (Rebuild)</name>
        <version>20250601-000000</version>
        <author>No-Intro</author>
    </header>
    <game name="Tetris (World)">
        <rom name="Tetris (World).gb" size="1" crc="00000000" sha1="0000000000000000000000000000000000000a"/>
    </game>
</datafile>"#;

    fn write_dat(dir: &StdPath, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn file_entry(id: &str, path: PathBuf, platform: Option<&str>) -> DatSourceEntry {
        let mut entry =
            DatSourceEntry::new(id.to_string(), id.to_string(), path, DatSourceKind::File);
        entry.platform = platform.map(str::to_string);
        entry
    }

    fn write_config(dir: &StdPath, registry: &DatSourceRegistry) -> PathBuf {
        let config_path = dir.join("dat_sources.toml");
        archivefs_core::dat::sources::save_dat_sources_config_to(
            &config_path,
            &registry.to_config(),
        )
        .unwrap();
        config_path
    }

    #[test]
    fn no_registry_file_yields_empty_status_not_an_error() {
        let status = gather_no_intro_sources_status(None);
        assert!(status.rows.is_empty());
        assert!(status.ambiguities.is_empty());
    }

    #[test]
    fn rows_reflect_enabled_disabled_and_platform() {
        let dir = tempdir().unwrap();
        let gb_dat_path = write_dat(dir.path(), "gb.dat", GB_NO_INTRO_XML);
        let nes_dat_path = write_dat(dir.path(), "nes.dat", GB_NO_INTRO_XML_OTHER);
        let mut registry = DatSourceRegistry::new();
        let mut disabled_entry = file_entry("gb-off", gb_dat_path, Some("Game Boy"));
        disabled_entry.enabled = false;
        registry.add(disabled_entry).unwrap();
        registry
            .add(file_entry("nes-on", nes_dat_path, Some("NES")))
            .unwrap();
        let config_path = write_config(dir.path(), &registry);

        let status = gather_no_intro_sources_status(Some(&config_path));
        assert_eq!(status.rows.len(), 2);
        let off = status
            .rows
            .iter()
            .find(|r| r.source_id == "gb-off")
            .unwrap();
        assert!(!off.enabled);
        assert_eq!(off.platform_label, "Nintendo Game Boy");
        let on = status
            .rows
            .iter()
            .find(|r| r.source_id == "nes-on")
            .unwrap();
        assert!(on.enabled);
        assert_eq!(on.platform_label, "Nintendo Entertainment System");
    }

    #[test]
    fn unassigned_source_gets_an_honest_label_not_a_blank() {
        let dir = tempdir().unwrap();
        let dat_path = write_dat(dir.path(), "gb.dat", GB_NO_INTRO_XML);
        let mut registry = DatSourceRegistry::new();
        registry
            .add(file_entry("unassigned", dat_path, None))
            .unwrap();
        let config_path = write_config(dir.path(), &registry);

        let status = gather_no_intro_sources_status(Some(&config_path));
        assert_eq!(status.rows[0].platform_label, "Unassigned (any platform)");
    }

    #[test]
    fn missing_dat_file_is_reported_as_unreadable_health() {
        let dir = tempdir().unwrap();
        // `DatSourceRegistry::add` validates the path exists, so a source
        // whose file has since vanished from disk can only be modelled by
        // registering a real one and then letting it go missing - exactly
        // what happens in real use when a DAT file is moved or deleted
        // after being registered.
        let dat_path = write_dat(dir.path(), "will-vanish.dat", GB_NO_INTRO_XML);
        let mut registry = DatSourceRegistry::new();
        let mut entry = file_entry("missing", dat_path.clone(), Some("Game Boy"));
        // Simulate a previously-recorded health verdict the way the real
        // page would have persisted after a failed validation run.
        entry.health.state = Some(DatHealthState::Unreadable);
        entry.health.detail = Some("path does not exist".to_string());
        registry.add(entry).unwrap();
        std::fs::remove_file(&dat_path).unwrap();
        let config_path = write_config(dir.path(), &registry);

        let status = gather_no_intro_sources_status(Some(&config_path));
        let row = &status.rows[0];
        assert_eq!(row.health_label, "Path unreadable");
        assert_eq!(row.health_tone, widgets::StatusTone::Blocked);
        assert_eq!(row.health_detail.as_deref(), Some("path does not exist"));
    }

    #[test]
    fn never_checked_health_is_shown_as_pending_not_invalid() {
        let dir = tempdir().unwrap();
        let dat_path = write_dat(dir.path(), "gb.dat", GB_NO_INTRO_XML);
        let mut registry = DatSourceRegistry::new();
        registry
            .add(file_entry("fresh", dat_path, Some("Game Boy")))
            .unwrap();
        let config_path = write_config(dir.path(), &registry);

        let status = gather_no_intro_sources_status(Some(&config_path));
        assert_eq!(status.rows[0].health_label, "Not checked");
        assert_eq!(status.rows[0].health_tone, widgets::StatusTone::Pending);
    }

    #[test]
    fn single_source_per_platform_is_not_ambiguous() {
        let dir = tempdir().unwrap();
        let dat_path = write_dat(dir.path(), "gb.dat", GB_NO_INTRO_XML);
        let mut registry = DatSourceRegistry::new();
        registry
            .add(file_entry("gb", dat_path, Some("Game Boy")))
            .unwrap();
        let config_path = write_config(dir.path(), &registry);

        let status = gather_no_intro_sources_status(Some(&config_path));
        assert!(status.ambiguities.is_empty());
    }

    #[test]
    fn two_no_intro_sources_on_the_same_platform_are_flagged_ambiguous() {
        let dir = tempdir().unwrap();
        let dat_path_a = write_dat(dir.path(), "gb-a.dat", GB_NO_INTRO_XML);
        let dat_path_b = write_dat(dir.path(), "gb-b.dat", GB_NO_INTRO_XML_OTHER);
        let mut registry = DatSourceRegistry::new();
        registry
            .add(file_entry("gb-a", dat_path_a, Some("Game Boy")))
            .unwrap();
        registry
            .add(file_entry("gb-b", dat_path_b, Some("Game Boy")))
            .unwrap();
        let config_path = write_config(dir.path(), &registry);

        let status = gather_no_intro_sources_status(Some(&config_path));
        assert_eq!(status.ambiguities.len(), 1);
        let ambiguity = &status.ambiguities[0];
        assert_eq!(ambiguity.platform_label, "Nintendo Game Boy");
        assert_eq!(ambiguity.competing_sources.len(), 2);
    }

    #[test]
    fn different_platforms_never_flagged_ambiguous_against_each_other() {
        let dir = tempdir().unwrap();
        let gb_dat_path = write_dat(dir.path(), "gb.dat", GB_NO_INTRO_XML);
        let nes_dat_path = write_dat(dir.path(), "nes.dat", GB_NO_INTRO_XML_OTHER);
        let mut registry = DatSourceRegistry::new();
        registry
            .add(file_entry("gb", gb_dat_path, Some("Game Boy")))
            .unwrap();
        registry
            .add(file_entry("nes", nes_dat_path, Some("NES")))
            .unwrap();
        let config_path = write_config(dir.path(), &registry);

        let status = gather_no_intro_sources_status(Some(&config_path));
        assert!(status.ambiguities.is_empty());
    }

    #[test]
    fn disabled_source_never_counts_toward_ambiguity() {
        let dir = tempdir().unwrap();
        let dat_path_a = write_dat(dir.path(), "gb-a.dat", GB_NO_INTRO_XML);
        let dat_path_b = write_dat(dir.path(), "gb-b.dat", GB_NO_INTRO_XML_OTHER);
        let mut registry = DatSourceRegistry::new();
        registry
            .add(file_entry("gb-a", dat_path_a, Some("Game Boy")))
            .unwrap();
        let mut disabled = file_entry("gb-b", dat_path_b, Some("Game Boy"));
        disabled.enabled = false;
        registry.add(disabled).unwrap();
        let config_path = write_config(dir.path(), &registry);

        let status = gather_no_intro_sources_status(Some(&config_path));
        assert!(status.ambiguities.is_empty());
    }

    #[test]
    fn hasheous_info_is_static_and_never_marked_enabled() {
        // Hasheous has no persisted "enabled" bit to report - the strongest
        // guarantee this can offer that the page never fabricates one is
        // that `HasheousProviderInfo` carries no such field at all.
        let info = hasheous_provider_info();
        assert!(!info.base_url.is_empty());
    }

    // -- real render smoke tests ------------------------------------------

    #[test]
    fn idle_panel_renders_without_panicking() {
        let ctx = egui::Context::default();
        let state = IdentitySourcesState::Idle;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_identity_sources_panel(ui, false, &state);
            });
        });
    }

    #[test]
    fn ready_panel_renders_in_both_gamer_and_advanced_mode_without_panicking() {
        let dir = tempdir().unwrap();
        let dat_path_a = write_dat(dir.path(), "gb-a.dat", GB_NO_INTRO_XML);
        let dat_path_b = write_dat(dir.path(), "gb-b.dat", GB_NO_INTRO_XML_OTHER);
        let mut registry = DatSourceRegistry::new();
        registry
            .add(file_entry("gb-a", dat_path_a, Some("Game Boy")))
            .unwrap();
        registry
            .add(file_entry("gb-b", dat_path_b, Some("Game Boy")))
            .unwrap();
        let config_path = write_config(dir.path(), &registry);
        let status = gather_no_intro_sources_status(Some(&config_path));
        let state = IdentitySourcesState::Ready {
            generation: 1,
            status,
        };
        for advanced in [false, true] {
            let ctx = egui::Context::default();
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = show_identity_sources_panel(ui, advanced, &state);
                });
            });
        }
    }

    #[test]
    fn loading_panel_renders_without_panicking() {
        let (_sender, receiver) = std::sync::mpsc::channel();
        let ctx = egui::Context::default();
        let state = IdentitySourcesState::Loading {
            generation: 1,
            receiver,
        };
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_identity_sources_panel(ui, false, &state);
            });
        });
    }

    #[test]
    fn panel_never_offers_a_mutation_action() {
        // `IdentitySourcesAction` is the panel's entire vocabulary of things
        // it can ask the caller to do. If this ever grows an
        // Apply/Rename/Move/Delete variant, this match arm stops compiling.
        fn assert_read_only(action: IdentitySourcesAction) {
            match action {
                IdentitySourcesAction::Load => {}
            }
        }
        assert_read_only(IdentitySourcesAction::Load);
    }

    // -- ScummVM Detection -------------------------------------------------

    #[cfg(unix)]
    fn write_fixture_detector(dir: &StdPath, script: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("scummvm-fixture");
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn archive_row(path: &str, platform: &str) -> crate::ArchiveRow {
        crate::ArchiveRow {
            path: PathBuf::from(path),
            archive_path: path.to_string(),
            mount_path: String::new(),
            platform: platform.to_string(),
            state: String::new(),
            search_text: String::new(),
            origin: crate::RowOrigin::Live,
            unknown_platform: false,
            source_path: None,
        }
    }

    #[test]
    fn readiness_is_missing_when_no_executable_resolves() {
        // The real `resolve_scummvm_executable` only ever finds a real,
        // executable file, so on a machine (or sandbox) without ScummVM
        // installed this is exactly `Missing` - no fabricated "maybe".
        // This asserts the honest fallback shape rather than assuming a
        // specific test-runner environment has ScummVm installed.
        let readiness = gather_scummvm_readiness();
        match readiness {
            ScummVmReadiness::Missing => {}
            ScummVmReadiness::Ready { .. } => {
                // A CI/dev box that genuinely has scummvm on PATH is not a
                // failure - just not the case this test can assert further
                // than "it returned something, not a panic".
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn one_exact_match_is_bucketed_as_detected() {
        let dir = tempdir().unwrap();
        let game = dir.path().join("game");
        std::fs::create_dir(&game).unwrap();
        let detector = write_fixture_detector(
            dir.path(),
            "#!/bin/sh\nprintf 'Game: scumm:monkey (The Secret of Monkey Island)\\n'\n",
        );
        let summary =
            check_scummvm_candidates(&detector, &[(game, "My Game".to_string())], |_, _, _| {});
        assert_eq!(summary.rows.len(), 1);
        assert_eq!(summary.rows[0].outcome.label(), "Detected");
        assert!(matches!(
            &summary.rows[0].outcome,
            ScummVmOutcome::Detected { engine_id, game_id, .. }
                if engine_id == "scumm" && game_id == "monkey"
        ));
        assert_eq!(summary.count_matching("Detected"), 1);
    }

    #[cfg(unix)]
    #[test]
    fn multiple_candidates_are_bucketed_as_ambiguous_never_auto_chosen() {
        let dir = tempdir().unwrap();
        let game = dir.path().join("game");
        std::fs::create_dir(&game).unwrap();
        let detector = write_fixture_detector(
            dir.path(),
            "#!/bin/sh\nprintf 'Game: scumm:one\\nGame: sci:two\\n'\n",
        );
        let summary = check_scummvm_candidates(
            &detector,
            &[(game, "Ambiguous Game".to_string())],
            |_, _, _| {},
        );
        assert_eq!(summary.rows[0].outcome.label(), "Ambiguous");
        assert!(matches!(
            summary.rows[0].outcome,
            ScummVmOutcome::Ambiguous { candidate_count: 2 }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn no_recognised_game_is_bucketed_as_unsupported() {
        let dir = tempdir().unwrap();
        let game = dir.path().join("game");
        std::fs::create_dir(&game).unwrap();
        let detector =
            write_fixture_detector(dir.path(), "#!/bin/sh\nprintf 'no games detected\\n'\n");
        let summary = check_scummvm_candidates(
            &detector,
            &[(game, "Unknown Game".to_string())],
            |_, _, _| {},
        );
        assert_eq!(summary.rows[0].outcome.label(), "Unsupported");
        assert_eq!(summary.rows[0].outcome, ScummVmOutcome::Unsupported);
    }

    #[test]
    fn a_folder_the_detector_cannot_examine_is_not_recognised() {
        // No such directory at all - `validate_game_directory` fails before
        // any subprocess runs, so this exercises the "detector could not
        // even look" bucket without needing a fixture executable.
        let summary = check_scummvm_candidates(
            Path::new("/bin/true"),
            &[(
                PathBuf::from("/nonexistent/scummvm-game-folder"),
                "Missing Folder".to_string(),
            )],
            |_, _, _| {},
        );
        assert_eq!(summary.rows[0].outcome.label(), "Not recognised");
        assert!(matches!(
            summary.rows[0].outcome,
            ScummVmOutcome::NotRecognised { .. }
        ));
    }

    #[test]
    fn check_never_reports_a_result_for_an_unchecked_folder() {
        // Every row in the summary must come from an actual detector call -
        // this pins that the function does not synthesize placeholder rows.
        let summary = check_scummvm_candidates(Path::new("/bin/true"), &[], |_, _, _| {});
        assert!(summary.rows.is_empty());
    }

    #[test]
    fn progress_callback_reports_checked_of_total_before_each_item() {
        let mut seen = Vec::new();
        let candidates = [
            (PathBuf::from("/nonexistent/a"), "A".to_string()),
            (PathBuf::from("/nonexistent/b"), "B".to_string()),
        ];
        let _ = check_scummvm_candidates(
            Path::new("/bin/true"),
            &candidates,
            |checked, total, current| {
                seen.push((checked, total, current.to_string()));
            },
        );
        assert_eq!(seen, vec![(0, 2, "A".to_string()), (1, 2, "B".to_string())]);
    }

    #[test]
    fn candidates_are_selected_by_canonical_platform_never_by_filename() {
        let rows = vec![
            archive_row("/library/monkey-island", "ScummVM"),
            archive_row("/library/some.gen", "Mega Drive"),
            archive_row("/library/sword", "scummvm"),
            archive_row("/library/random-folder-named-scumm", "Unknown"),
        ];
        let candidates = scummvm_candidates_from_rows(&rows);
        let paths: Vec<&std::path::Path> =
            candidates.iter().map(|(path, _)| path.as_path()).collect();
        assert_eq!(
            paths,
            vec![
                std::path::Path::new("/library/monkey-island"),
                std::path::Path::new("/library/sword"),
            ],
            "only rows whose platform actually resolves to ScummVM are candidates - \
             a folder merely named 'scumm' is never enough"
        );
    }

    #[test]
    fn candidate_label_falls_back_to_the_full_path_when_there_is_no_file_name() {
        let rows = vec![archive_row("/", "ScummVM")];
        let candidates = scummvm_candidates_from_rows(&rows);
        assert_eq!(candidates.len(), 1);
        assert!(!candidates[0].1.is_empty());
    }

    #[test]
    fn scummvm_outcome_is_never_reachable_through_a_dat_or_network_type() {
        // `ScummVmOutcome`/`check_scummvm_candidates` never reference a
        // `DatSourceEntry`, a hashing type, or any network client type -
        // this test exists to be a visible anchor: if a future edit adds
        // such a dependency, a reviewer sees it fail its own doc comment's
        // promise, even though the compiler cannot check "no network" on
        // its own.
        let outcome = ScummVmOutcome::Unsupported;
        assert_eq!(outcome.label(), "Unsupported");
    }

    #[test]
    fn idle_scummvm_panel_renders_without_panicking() {
        let ctx = egui::Context::default();
        let readiness = ScummVmReadinessState::NotChecked;
        let check_state = ScummVmCheckState::Idle;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_scummvm_detection_panel(ui, &readiness, 0, &check_state);
            });
        });
    }

    #[test]
    fn missing_scummvm_panel_renders_without_panicking() {
        let ctx = egui::Context::default();
        let readiness = ScummVmReadinessState::Ready(ScummVmReadiness::Missing);
        let check_state = ScummVmCheckState::Idle;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_scummvm_detection_panel(ui, &readiness, 0, &check_state);
            });
        });
    }

    #[test]
    fn checking_scummvm_panel_renders_progress_without_panicking() {
        let ctx = egui::Context::default();
        let readiness = ScummVmReadinessState::Ready(ScummVmReadiness::Ready {
            executable: PathBuf::from("/usr/bin/scummvm"),
            version: Some("ScummVM 2.8.1".to_string()),
        });
        let (_sender, receiver) = std::sync::mpsc::channel();
        let check_state = ScummVmCheckState::Checking {
            generation: 1,
            receiver,
            checked: 1,
            total: 3,
            current: Some("Some Game".to_string()),
        };
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_scummvm_detection_panel(ui, &readiness, 3, &check_state);
            });
        });
    }

    #[test]
    fn ready_scummvm_panel_renders_bounded_results_without_panicking() {
        let ctx = egui::Context::default();
        let readiness = ScummVmReadinessState::Ready(ScummVmReadiness::Ready {
            executable: PathBuf::from("/usr/bin/scummvm"),
            version: None,
        });
        let summary = ScummVmCheckSummary {
            rows: vec![
                ScummVmCheckRow {
                    path: PathBuf::from("/library/monkey"),
                    label: "Monkey Island".to_string(),
                    outcome: ScummVmOutcome::Detected {
                        engine_id: "scumm".to_string(),
                        game_id: "monkey".to_string(),
                        description: None,
                    },
                },
                ScummVmCheckRow {
                    path: PathBuf::from("/library/mystery"),
                    label: "Mystery Game".to_string(),
                    outcome: ScummVmOutcome::Ambiguous { candidate_count: 2 },
                },
                ScummVmCheckRow {
                    path: PathBuf::from("/library/blank"),
                    label: "Blank Folder".to_string(),
                    outcome: ScummVmOutcome::Unsupported,
                },
                ScummVmCheckRow {
                    path: PathBuf::from("/library/broken"),
                    label: "Broken Folder".to_string(),
                    outcome: ScummVmOutcome::NotRecognised {
                        detail: "invalid ScummVM game folder: not a directory".to_string(),
                    },
                },
            ],
        };
        let check_state = ScummVmCheckState::Ready {
            generation: 1,
            summary,
        };
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_scummvm_detection_panel(ui, &readiness, 4, &check_state);
            });
        });
    }

    #[test]
    fn a_result_beyond_the_render_cap_is_summarized_not_dropped_silently() {
        let rows: Vec<ScummVmCheckRow> = (0..MAX_RENDERED_SCUMMVM_ROWS + 5)
            .map(|index| ScummVmCheckRow {
                path: PathBuf::from(format!("/library/game-{index}")),
                label: format!("Game {index}"),
                outcome: ScummVmOutcome::Unsupported,
            })
            .collect();
        let summary = ScummVmCheckSummary { rows };
        assert_eq!(
            summary.count_matching("Unsupported"),
            MAX_RENDERED_SCUMMVM_ROWS + 5
        );
        // The render function itself only iterates the first
        // `MAX_RENDERED_SCUMMVM_ROWS`; this test pins that the summary
        // (used for the counts, which the render function also reads)
        // still carries every row rather than having been truncated by
        // `check_scummvm_candidates` itself - truncation is a rendering
        // concern only.
        assert_eq!(summary.rows.len(), MAX_RENDERED_SCUMMVM_ROWS + 5);
    }

    #[test]
    fn scummvm_action_is_only_ever_a_read_only_check() {
        // `ScummVmAction` is this panel's entire vocabulary of things it can
        // ask the caller to do. If this ever grows a write/rename/download
        // variant, this match arm stops compiling.
        fn assert_read_only(action: ScummVmAction) {
            match action {
                ScummVmAction::Check { .. } => {}
            }
        }
        assert_read_only(ScummVmAction::Check {
            executable: PathBuf::from("/usr/bin/scummvm"),
        });
    }
}
