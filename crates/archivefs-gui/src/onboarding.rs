//! First-run onboarding: a thin, resumable step tracker over the real
//! Sources, DAT Sources, and Emulator Setup pages.
//!
//! This module deliberately owns almost no state of its own. It tracks
//! exactly one thing - which step of a fixed 5-step tour the user is on,
//! or whether they finished/skipped it - and persists that as a single
//! small sidecar file, following the same pattern already established by
//! `retroarch_core_directory_override.txt` (`main.rs`'s
//! `load_retroarch_core_directory_override_at`/`save_retroarch_core_directory_override_at`):
//! a plain-text file under the app config directory, outside
//! `archivefs_core::Config`, loaded best-effort at startup, never blocking
//! if absent or unreadable.
//!
//! Every step's body is the *real* page, called through the exact same
//! `ArchiveFsApp` methods normal navigation already uses
//! (`show_sources_page`, `show_dat_sources_page`, `show_emulator_setup_page`)
//! - this module adds no second source/DAT/emulator rendering path, and no
//! new backend calls. See `docs/FIRST_RUN_ONBOARDING_PLAN.md` for the full
//! design this implements.

use super::*;

// --------------------------------------------------------------------
// Pure state model - no `ArchiveFsApp` dependency, fully unit-testable.
// --------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OnboardingStep {
    Welcome,
    AddSource,
    DatSetup,
    EmulatorSetup,
    Verify,
}

pub(crate) const ONBOARDING_STEP_COUNT: usize = 5;

impl OnboardingStep {
    const ORDER: [OnboardingStep; ONBOARDING_STEP_COUNT] = [
        OnboardingStep::Welcome,
        OnboardingStep::AddSource,
        OnboardingStep::DatSetup,
        OnboardingStep::EmulatorSetup,
        OnboardingStep::Verify,
    ];

    /// 1-based position for "Step X of 5" display.
    pub(crate) fn position(self) -> usize {
        Self::ORDER
            .iter()
            .position(|step| *step == self)
            .map_or(1, |index| index + 1)
    }

    pub(crate) fn next(self) -> Option<OnboardingStep> {
        let index = Self::ORDER.iter().position(|step| *step == self)?;
        Self::ORDER.get(index + 1).copied()
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            OnboardingStep::Welcome => "Welcome to EmuWiz",
            OnboardingStep::AddSource => "Add a source",
            OnboardingStep::DatSetup => "Optional: DAT / identification setup",
            OnboardingStep::EmulatorSetup => "Emulator discovery & readiness",
            OnboardingStep::Verify => "Verify & finish",
        }
    }

    fn wire_name(self) -> &'static str {
        match self {
            OnboardingStep::Welcome => "welcome",
            OnboardingStep::AddSource => "add_source",
            OnboardingStep::DatSetup => "dat_setup",
            OnboardingStep::EmulatorSetup => "emulator_setup",
            OnboardingStep::Verify => "verify",
        }
    }

    fn from_wire_name(value: &str) -> Option<OnboardingStep> {
        Self::ORDER
            .into_iter()
            .find(|step| step.wire_name() == value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OnboardingState {
    NotStarted,
    InProgress(OnboardingStep),
    Skipped,
    Completed,
}

impl OnboardingState {
    /// `"not_started"` / `"in_progress:<step>"` / `"skipped"` / `"completed"`.
    /// Anything else (a future version's format, truncation, a stray
    /// newline-only file) parses as `NotStarted` - the same "never treat a
    /// malformed sidecar as an error" rule the RetroArch override follows.
    pub(crate) fn parse(text: &str) -> OnboardingState {
        let text = text.trim();
        match text {
            "not_started" => OnboardingState::NotStarted,
            "skipped" => OnboardingState::Skipped,
            "completed" => OnboardingState::Completed,
            _ => text
                .strip_prefix("in_progress:")
                .and_then(OnboardingStep::from_wire_name)
                .map_or(OnboardingState::NotStarted, OnboardingState::InProgress),
        }
    }

    pub(crate) fn serialize(self) -> String {
        match self {
            OnboardingState::NotStarted => "not_started".to_string(),
            OnboardingState::Skipped => "skipped".to_string(),
            OnboardingState::Completed => "completed".to_string(),
            OnboardingState::InProgress(step) => format!("in_progress:{}", step.wire_name()),
        }
    }
}

// --------------------------------------------------------------------
// Persistence (GUI-only sidecar, mirrors `retroarch_core_directory_override`)
// --------------------------------------------------------------------

pub(crate) fn onboarding_state_path() -> Option<PathBuf> {
    archivefs_core::app_dirs::config_path("onboarding_state.txt").ok()
}

/// Reads onboarding state from an explicit path. Never fails: a missing or
/// unreadable file, or one whose contents don't parse, all read as
/// `NotStarted` rather than blocking startup or surfacing an error.
pub(crate) fn load_onboarding_state_at(path: &Path) -> OnboardingState {
    std::fs::read_to_string(path).map_or(OnboardingState::NotStarted, |text| {
        OnboardingState::parse(&text)
    })
}

/// Writes onboarding state at an explicit path. Best-effort: a persistence
/// failure never blocks the in-memory transition from taking effect for
/// the running session.
pub(crate) fn save_onboarding_state_at(path: &Path, state: OnboardingState) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, state.serialize());
}

pub(crate) fn load_onboarding_state() -> OnboardingState {
    onboarding_state_path()
        .as_deref()
        .map_or(OnboardingState::NotStarted, load_onboarding_state_at)
}

pub(crate) fn save_onboarding_state(state: OnboardingState) {
    if let Some(path) = onboarding_state_path() {
        save_onboarding_state_at(&path, state);
    }
}

// --------------------------------------------------------------------
// Overlay rendering - the only part that touches `ArchiveFsApp`/egui.
// --------------------------------------------------------------------

impl ArchiveFsApp {
    /// Opens onboarding automatically exactly once per session, and only
    /// on a genuine first run: reuses `missing_config_is_first_run`
    /// (the same predicate the Setup/Diagnostics welcome banner and
    /// Home's fresh-install banner already agree on) rather than adding a
    /// third, possibly-diverging notion of "is this a first run".
    /// Runs at the point `config_previously_confirmed` is first known to
    /// be settled for this session (right after diagnostics complete),
    /// never before.
    pub(crate) fn maybe_auto_open_onboarding(&mut self) {
        if self.onboarding_auto_open_checked {
            return;
        }
        self.onboarding_auto_open_checked = true;
        if self.onboarding_state == OnboardingState::NotStarted
            && missing_config_is_first_run(self.config_previously_confirmed)
        {
            self.onboarding_state = OnboardingState::InProgress(OnboardingStep::Welcome);
            save_onboarding_state(self.onboarding_state);
            self.tools_overlay = ToolsOverlay::Onboarding;
        }
    }

    /// "Run setup again": resets onboarding progress only. Never touches
    /// `Config`, source folders, DAT registrations, or emulator profiles -
    /// each step's body will simply render whatever already-configured
    /// state those pages already have, exactly as the design requires.
    pub(crate) fn restart_onboarding(&mut self) {
        self.onboarding_state = OnboardingState::InProgress(OnboardingStep::Welcome);
        save_onboarding_state(self.onboarding_state);
        self.tools_overlay = ToolsOverlay::Onboarding;
    }

    pub(crate) fn onboarding_advance_from(&mut self, step: OnboardingStep) {
        self.onboarding_state = match step.next() {
            Some(next) => OnboardingState::InProgress(next),
            None => {
                self.tools_overlay = ToolsOverlay::None;
                OnboardingState::Completed
            }
        };
        save_onboarding_state(self.onboarding_state);
    }

    pub(crate) fn onboarding_skip_entirely(&mut self) {
        self.onboarding_state = OnboardingState::Skipped;
        save_onboarding_state(self.onboarding_state);
        self.tools_overlay = ToolsOverlay::None;
    }

    pub(crate) fn onboarding_has_source(&self) -> bool {
        self.gui_config
            .source_roots()
            .map(|roots| roots.len())
            .unwrap_or(0)
            > 0
    }

    pub(crate) fn onboarding_dat_source_count(&self) -> usize {
        self.dat_sources_page
            .as_ref()
            .map(|page| page.registered_source_count())
            .unwrap_or(0)
    }

    pub(crate) fn show_onboarding_overlay(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        let OnboardingState::InProgress(step) = self.onboarding_state else {
            // Defensive only: this overlay is only ever entered while
            // `InProgress`. If state has drifted (e.g. persisted state
            // changed underneath a long-running session), fail safely by
            // closing the overlay rather than rendering a stale step.
            self.tools_overlay = ToolsOverlay::None;
            return;
        };
        let mut skip_entirely_clicked = false;
        ui.horizontal(|ui| {
            ui.heading(format!(
                "First-time setup - step {} of {}",
                step.position(),
                ONBOARDING_STEP_COUNT
            ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Skip setup entirely").clicked() {
                    skip_entirely_clicked = true;
                }
            });
        });
        if skip_entirely_clicked {
            self.onboarding_skip_entirely();
            return;
        }
        ui.add_space(4.0);
        ui.strong(step.title());
        ui.add_space(8.0);
        match step {
            OnboardingStep::Welcome => self.show_onboarding_welcome_step(ui, step),
            OnboardingStep::AddSource => self.show_onboarding_add_source_step(ui, context, step),
            OnboardingStep::DatSetup => self.show_onboarding_dat_step(ui, step),
            OnboardingStep::EmulatorSetup => self.show_onboarding_emulator_step(ui, context, step),
            OnboardingStep::Verify => self.show_onboarding_verify_step(ui, step),
        }
    }

    fn show_onboarding_welcome_step(&mut self, ui: &mut egui::Ui, step: OnboardingStep) {
        ui.label(
            "EmuWiz will never do the following without an explicit, reviewed confirmation step:",
        );
        ui.label("  \u{2022} rename, move, or delete a ROM");
        ui.label("  \u{2022} download anything from the internet");
        ui.label("  \u{2022} configure an emulator on your behalf");
        ui.label("  \u{2022} require a DAT provider account or sign-up");
        ui.add_space(8.0);
        ui.label(
            "A \"source\" is just a folder where your games already live - EmuWiz scans it \
             without changing any file inside. DAT catalogues, RomM, and ES-DE publishing are \
             all optional extras you can add later; nothing here is required to use your \
             library.",
        );
        ui.add_space(12.0);
        if ui.button("Continue").clicked() {
            self.onboarding_advance_from(step);
        }
    }

    fn show_onboarding_add_source_step(
        &mut self,
        ui: &mut egui::Ui,
        context: &egui::Context,
        step: OnboardingStep,
    ) {
        ui.label(
            "Choose the folder where your games are stored. EmuWiz scans it without changing \
             any files inside.",
        );
        ui.add_space(8.0);
        self.show_sources_page(context, ui, SourcesTab::Libraries);
        ui.add_space(12.0);
        let has_source = self.onboarding_has_source();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(has_source, egui::Button::new("Continue"))
                .clicked()
            {
                self.onboarding_advance_from(step);
            }
            if ui.button("Skip for now").clicked() {
                self.onboarding_advance_from(step);
            }
        });
    }

    fn show_onboarding_dat_step(&mut self, ui: &mut egui::Ui, step: OnboardingStep) {
        ui.label(
            "A DAT is a trusted list of known-good game files, used only to verify your \
             collection - it is entirely optional. Adding one never requires a provider \
             account or network access; most people can safely skip this.",
        );
        ui.add_space(8.0);
        self.show_dat_sources_page(ui);
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.button("Continue").clicked() {
                self.onboarding_advance_from(step);
            }
            if ui.button("Skip for now").clicked() {
                self.onboarding_advance_from(step);
            }
        });
    }

    fn show_onboarding_emulator_step(
        &mut self,
        ui: &mut egui::Ui,
        context: &egui::Context,
        step: OnboardingStep,
    ) {
        ui.label(
            "This shows what EmuWiz can find on this machine and which platforms are ready to \
             launch. Nothing here is installed or configured automatically - you can always \
             continue even if some systems still need manual setup.",
        );
        ui.add_space(8.0);
        self.show_emulator_setup_page(ui, context);
        ui.add_space(12.0);
        if ui.button("Continue").clicked() {
            self.onboarding_advance_from(step);
        }
    }

    fn show_onboarding_verify_step(&mut self, ui: &mut egui::Ui, step: OnboardingStep) {
        if self.onboarding_dat_source_count() > 0 {
            ui.label(
                "Verify compares your library against the DAT catalogue(s) you added - it only \
                 reads files, it never renames, moves, or deletes anything.",
            );
            ui.add_space(8.0);
            self.show_dat_sources_page(ui);
            ui.add_space(12.0);
        } else {
            ui.label(
                "No DAT catalogue was added, so there is nothing to verify yet - that's fine. \
                 You can add one later from DAT Sources whenever you want.",
            );
            ui.add_space(12.0);
        }
        if ui.button("Finish").clicked() {
            self.onboarding_advance_from(step);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_order_is_exactly_the_approved_five_steps_in_order() {
        assert_eq!(
            OnboardingStep::ORDER,
            [
                OnboardingStep::Welcome,
                OnboardingStep::AddSource,
                OnboardingStep::DatSetup,
                OnboardingStep::EmulatorSetup,
                OnboardingStep::Verify,
            ]
        );
    }

    #[test]
    fn position_is_one_based() {
        assert_eq!(OnboardingStep::Welcome.position(), 1);
        assert_eq!(OnboardingStep::Verify.position(), 5);
    }

    #[test]
    fn next_walks_through_every_step_then_stops() {
        let mut step = OnboardingStep::Welcome;
        let mut visited = vec![step];
        while let Some(next) = step.next() {
            visited.push(next);
            step = next;
        }
        assert_eq!(visited, OnboardingStep::ORDER.to_vec());
        assert_eq!(OnboardingStep::Verify.next(), None);
    }

    #[test]
    fn state_round_trips_through_serialize_and_parse_for_every_variant() {
        let states = [
            OnboardingState::NotStarted,
            OnboardingState::Skipped,
            OnboardingState::Completed,
            OnboardingState::InProgress(OnboardingStep::Welcome),
            OnboardingState::InProgress(OnboardingStep::AddSource),
            OnboardingState::InProgress(OnboardingStep::DatSetup),
            OnboardingState::InProgress(OnboardingStep::EmulatorSetup),
            OnboardingState::InProgress(OnboardingStep::Verify),
        ];
        for state in states {
            assert_eq!(OnboardingState::parse(&state.serialize()), state);
        }
    }

    #[test]
    fn unrecognised_text_parses_as_not_started() {
        assert_eq!(
            OnboardingState::parse("nonsense"),
            OnboardingState::NotStarted
        );
        assert_eq!(OnboardingState::parse(""), OnboardingState::NotStarted);
        assert_eq!(
            OnboardingState::parse("in_progress:not_a_real_step"),
            OnboardingState::NotStarted
        );
        assert_eq!(
            OnboardingState::parse("in_progress:"),
            OnboardingState::NotStarted
        );
    }

    #[test]
    fn parse_trims_surrounding_whitespace_and_a_trailing_newline() {
        assert_eq!(
            OnboardingState::parse("  completed  \n"),
            OnboardingState::Completed
        );
        assert_eq!(
            OnboardingState::parse("in_progress:emulator_setup\n"),
            OnboardingState::InProgress(OnboardingStep::EmulatorSetup)
        );
    }

    #[test]
    fn a_missing_file_loads_as_not_started() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("onboarding_state.txt");
        assert!(!path.exists());
        assert_eq!(load_onboarding_state_at(&path), OnboardingState::NotStarted);
    }

    #[test]
    fn save_then_load_round_trips_at_an_explicit_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/onboarding_state.txt");
        save_onboarding_state_at(&path, OnboardingState::InProgress(OnboardingStep::DatSetup));
        assert!(
            path.exists(),
            "save must create the file (and any parent directory)"
        );
        assert_eq!(
            load_onboarding_state_at(&path),
            OnboardingState::InProgress(OnboardingStep::DatSetup)
        );
    }

    #[test]
    fn a_later_save_overwrites_the_earlier_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("onboarding_state.txt");
        save_onboarding_state_at(&path, OnboardingState::InProgress(OnboardingStep::Welcome));
        save_onboarding_state_at(&path, OnboardingState::Completed);
        assert_eq!(load_onboarding_state_at(&path), OnboardingState::Completed);
    }
}
