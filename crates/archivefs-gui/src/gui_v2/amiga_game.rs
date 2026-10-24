//! Read-only Amiga summary in Game Details: format, machine, disk count and a
//! readiness checklist from `archivefs_core::amiga_game_readiness`, with links
//! to the existing setup, firmware, multi-disk and trainer pages. This card
//! never offers Play; the launch-readiness card above it owns that action.
use super::library::Game;
use super::routes::{Route, Section};
use crate::multidisc_launch_view::DiscGuidance;
use crate::ui::components::{StatusTone, card, status_badge};
use archivefs_core::amiga_cd_evidence::AmigaCdMachine;
use archivefs_core::amiga_game_readiness::{
    AmigaCheckKind, AmigaCheckState, AmigaDiskSetFacts, AmigaEnvironmentFacts, AmigaFixRoute,
    AmigaGameReadiness, assess_amiga_game, machine_for_platform, machine_label,
    observe_amiga_environment,
};
use archivefs_core::patch_manager::AmigaProfileDiscoveryRoots;
use eframe::egui::{self, RichText};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

pub(super) fn machine(game: &Game) -> Option<AmigaCdMachine> {
    machine_for_platform(&game.platform)
}

#[derive(Default)]
struct Panel {
    key: Option<(i64, PathBuf)>,
    facts: Option<AmigaEnvironmentFacts>,
    job: Option<Receiver<AmigaEnvironmentFacts>>,
}

impl Panel {
    fn select(&mut self, key: (i64, PathBuf), ctx: &egui::Context) {
        if self.key.as_ref() != Some(&key) {
            self.key = Some(key);
            self.facts = None;
            self.job = None;
            self.start(ctx);
        }
    }

    fn start(&mut self, ctx: &egui::Context) {
        let Some((_, path)) = self.key.clone() else {
            return;
        };
        let (tx, receiver) = mpsc::channel();
        self.job = Some(receiver);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let facts = std::panic::catch_unwind(|| {
                let roots = AmigaProfileDiscoveryRoots::from_environment();
                observe_amiga_environment(roots.as_ref(), &path, &AtomicBool::new(false))
            })
            .unwrap_or_default();
            let _ = tx.send(facts);
            ctx.request_repaint();
        });
    }

    fn poll(&mut self) {
        let Some(job) = &self.job else { return };
        match job.try_recv() {
            Ok(facts) => {
                self.facts = Some(facts);
                self.job = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.facts = Some(AmigaEnvironmentFacts::default());
                self.job = None;
            }
        }
    }
}

pub(super) fn disk_facts(guidance: Option<&DiscGuidance>) -> Option<AmigaDiskSetFacts> {
    guidance.map(|guidance| AmigaDiskSetFacts {
        count: guidance.discs.len(),
        order_verified: guidance.order_verified,
        problems: guidance.problems.len(),
    })
}

/// Shows the card for an Amiga, CD32 or CDTV entry and returns where the
/// person asked to go. Reads emulator settings in the background.
pub(super) fn show(ui: &mut egui::Ui, game: &Game, discs: Option<&DiscGuidance>) -> Option<Route> {
    let machine = machine(game)?;
    let shared = ui.ctx().data_mut(|data| {
        data.get_temp_mut_or_default::<Arc<Mutex<Panel>>>(egui::Id::new("amiga_game_panel"))
            .clone()
    });
    let mut panel = shared.lock().unwrap_or_else(|poison| poison.into_inner());
    panel.select(
        (game.archive.id, game.archive.absolute_path.clone()),
        ui.ctx(),
    );
    panel.poll();
    let readiness = assess_amiga_game(
        machine,
        &game.archive.absolute_path,
        game.inspection_incomplete,
        disk_facts(discs),
        panel.facts.as_ref(),
    );
    let (fix, recheck) = render(ui, &readiness, panel.job.is_some());
    if recheck {
        panel.start(ui.ctx());
    }
    fix.map(|fix| route(fix, game.archive.id))
}

pub(super) fn route(fix: AmigaFixRoute, game_id: i64) -> Route {
    use super::launch_readiness_summary::ReadinessAction;
    let action = match fix {
        AmigaFixRoute::EmulatorSetup => ReadinessAction::SetUpEmulator,
        AmigaFixRoute::Firmware => ReadinessAction::CheckFirmware,
        AmigaFixRoute::MultiDisk => ReadinessAction::ReviewDiscSet,
        AmigaFixRoute::Trainers => {
            return Route::Task {
                section: Section::Mods,
                game: game_id,
            };
        }
    };
    super::native_workflows::route_for_action(action, game_id)
        .expect("setup, firmware and disk-set actions always have a page")
}

pub(super) fn headline(readiness: &AmigaGameReadiness) -> String {
    let mut parts = vec![
        machine_label(readiness.machine).to_string(),
        readiness.media.label().to_string(),
    ];
    match readiness.disk_count {
        Some(count) if count > 1 => parts.push(format!("{count} disks")),
        _ if readiness.media.is_floppy() => parts.push("1 disk".into()),
        _ => {}
    }
    parts.join(" · ")
}

fn row_label(kind: AmigaCheckKind) -> &'static str {
    match kind {
        AmigaCheckKind::Media => "Format",
        AmigaCheckKind::Emulator => "Emulator",
        AmigaCheckKind::Machine => "Machine",
        AmigaCheckKind::Kickstart => "Kickstart",
        AmigaCheckKind::Disks => "Disks",
        AmigaCheckKind::Whdload => "WHDLoad",
        AmigaCheckKind::Trainers => "Trainers",
        AmigaCheckKind::Play => "Play",
    }
}

fn tone(kind: AmigaCheckKind, state: AmigaCheckState) -> StatusTone {
    match (kind, state) {
        (AmigaCheckKind::Trainers, _) => StatusTone::Info,
        (_, AmigaCheckState::Ready) => StatusTone::Success,
        (_, AmigaCheckState::Warning) => StatusTone::Warning,
        (_, AmigaCheckState::Blocked) => StatusTone::Blocked,
    }
}

pub(super) fn fix_label(fix: AmigaFixRoute) -> &'static str {
    match fix {
        AmigaFixRoute::EmulatorSetup => "Set up emulator",
        AmigaFixRoute::Firmware => "Check firmware",
        AmigaFixRoute::MultiDisk => "Review disk set",
        AmigaFixRoute::Trainers => "Open Mods & Cheats",
    }
}

/// Returns the chosen fix page and whether "Check again" was pressed.
pub(super) fn render(
    ui: &mut egui::Ui,
    readiness: &AmigaGameReadiness,
    reading: bool,
) -> (Option<AmigaFixRoute>, bool) {
    let mut fix = None;
    let mut recheck = false;
    card(ui, |ui| {
        ui.label(RichText::new("Amiga game").strong());
        ui.label(headline(readiness));
        if reading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading emulator settings…");
            });
        }
        for check in &readiness.checks {
            ui.horizontal_wrapped(|ui| {
                status_badge(ui, row_label(check.kind), tone(check.kind, check.state));
                ui.label(&check.detail);
                if let Some(route) = check.route
                    && ui.small_button(fix_label(route)).clicked()
                {
                    fix = Some(route);
                }
            });
        }
        if !reading && ui.small_button("Check again").clicked() {
            recheck = true;
        }
    });
    (fix, recheck)
}

#[cfg(test)]
#[path = "amiga_game_tests.rs"]
mod tests;
