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
    AmigaGameReadiness, AmigaMediaKind, assess_amiga_game, machine_for_platform, machine_label,
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
                let mut facts =
                    observe_amiga_environment(roots.as_ref(), &path, &AtomicBool::new(false));
                if AmigaMediaKind::from_path(&path) == AmigaMediaKind::Adf {
                    facts.adf_identity = Some(
                        archivefs_core::launch::current_adf_launch_identity(&path).map(
                            |identity| {
                                format!(
                                    "{}; it counts as a complete single-disk game because {}",
                                    identity.entry_name,
                                    identity.standalone.describe()
                                )
                            },
                        ),
                    );
                }
                facts
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
        missing: guidance.missing_media.len(),
        this_disk_unavailable: guidance.discs.iter().any(|disc| {
            disc.this_entry
                && disc.path.as_deref().is_none_or(|path| {
                    std::fs::symlink_metadata(path).map_or(true, |m| !m.is_file())
                })
        }),
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
    let mut readiness = assess_amiga_game(
        machine,
        &game.archive.absolute_path,
        game.inspection_incomplete,
        disk_facts(discs),
        panel.facts.as_ref(),
    );
    apply_prepared_copy(
        &mut readiness,
        prepared_copy(
            &game.archive.absolute_path,
            &game.platform,
            // The Launch page plans from a prepared copy only for a game
            // with current evidence and no identity of its own.
            game.inspection_incomplete || game.identified,
        ),
    );
    let (fix, recheck) = render(ui, &readiness, panel.job.is_some());
    if recheck {
        panel.start(ui.ctx());
    }
    fix.map(|fix| route(fix, game.archive.id))
}

/// What game recovery says about a disc stored in a ZIP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PreparedCopy {
    /// Game recovery has no preparation for this kind of entry.
    NotApplicable,
    /// It could be prepared, but no usable prepared copy exists: none was
    /// made, or the ZIP or the copy has changed since.
    Missing,
    /// A checked prepared copy serves this exact file; Play starts it.
    Ready,
}

/// Asks the same lookup the Launch page plans from, so the card and Play
/// never disagree about whether a prepared copy is usable.
///
/// `launch_ignores_copy`: the Launch page does not plan from a prepared copy
/// for this game (its last scan could not finish, or it has an identity of
/// its own). The card is then left exactly as it was, "scan it again"
/// warning included.
pub(super) fn prepared_copy(
    path: &std::path::Path,
    recorded_platform: &str,
    launch_ignores_copy: bool,
) -> PreparedCopy {
    let platform = archivefs_core::canonical_platform_for_alias(recorded_platform);
    if launch_ignores_copy || !archivefs_core::recovery::preparation_may_apply(path, platform) {
        PreparedCopy::NotApplicable
    } else if crate::emulator_setup::prepared_media::for_game(path, Some(recorded_platform))
        .is_some()
    {
        PreparedCopy::Ready
    } else {
        PreparedCopy::Missing
    }
}

pub(super) const PREPARED_COPY_READY: &str = "Ready from a prepared copy. EmuWiz extracted this disc as new files and checked them against the ZIP; Play starts that copy. The ZIP was not changed.";
pub(super) const PREPARED_COPY_PLAY: &str = "Start it from the Launch page with RetroArch (PUAE). The game is not identified, so the Launch page asks you to confirm the system, and it still checks the emulator, its firmware and the prepared files.";

/// Brings the card's "stored in a ZIP" rows in line with game recovery. Only
/// the refusal for a CD32 ZIP is touched; every other entry keeps the
/// checklist it was given.
pub(super) fn apply_prepared_copy(readiness: &mut AmigaGameReadiness, copy: PreparedCopy) {
    // Game recovery prepares CD32 discs only.
    let zipped_disc =
        readiness.media == AmigaMediaKind::ZipArchive && readiness.machine == AmigaCdMachine::Cd32;
    if !zipped_disc || copy == PreparedCopy::NotApplicable {
        return;
    }
    let Some(media) = readiness
        .checks
        .iter_mut()
        .find(|check| check.kind == AmigaCheckKind::Media)
    else {
        return;
    };
    if media.state != AmigaCheckState::Blocked {
        return;
    }
    match copy {
        PreparedCopy::Ready => {
            media.state = AmigaCheckState::Ready;
            media.detail = PREPARED_COPY_READY.into();
            if let Some(play) = readiness
                .checks
                .iter_mut()
                .find(|check| check.kind == AmigaCheckKind::Play)
            {
                play.state = AmigaCheckState::Warning;
                play.detail = PREPARED_COPY_PLAY.into();
            }
            // These rows describe FS-UAE and Amiberry, which do not start a
            // prepared copy; the Launch page checks RetroArch for it.
            readiness.checks.retain(|check| {
                !matches!(
                    check.kind,
                    AmigaCheckKind::Emulator | AmigaCheckKind::Machine | AmigaCheckKind::Kickstart
                )
            });
        }
        PreparedCopy::Missing => {
            media.detail = format!(
                "{} discs are not started from inside a ZIP. Game recovery, on this page, can prepare a checked playable copy as new files; the ZIP is not changed.",
                machine_label(readiness.machine)
            );
        }
        PreparedCopy::NotApplicable => {}
    }
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
