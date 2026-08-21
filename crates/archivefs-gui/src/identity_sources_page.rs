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

use std::path::Path;

use eframe::egui;

use archivefs_core::dat::sources::{DatHealthState, DatSourceEntry, DatSourceRegistry};
use archivefs_core::identity_source::hasheous::client::HASHEOUS_DEFAULT_BASE_URL;
use archivefs_core::identity_source::no_intro::{NoIntroSourceSelection, select_no_intro_source};

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
}
