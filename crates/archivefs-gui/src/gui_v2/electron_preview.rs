//! Electron/BBC family inspection over existing validators. Workers only read;
//! this view neither changes library identity nor supplies a launch candidate.
use super::library::Game;
use crate::ui::{
    components::{StatusTone, card, status_badge},
    theme,
};
use archivefs_core::game_identity::{IdentityKind, IdentityStatus};
use archivefs_core::launch::elkulator::{
    self, ElkulatorMediaFormat,
    inspection::{ElkulatorInspection, format_from_path, inspect_media},
};
use archivefs_core::launch::native_support::ExecutableDiscovery;
use eframe::egui;
use std::{
    ffi::OsString,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, TryRecvError},
    },
    time::{Duration, Instant},
};

const RECHECK_INTERVAL: Duration = Duration::from_secs(2);

pub(super) fn applies(game: &Game) -> bool {
    matches!(game.platform.as_str(), "Acorn Electron" | "BBC Micro")
        || format_from_path(&game.archive.absolute_path) != ElkulatorMediaFormat::Unknown
}

fn platform_message(game: &Game) -> &'static str {
    let evidence = game
        .archive
        .identity_report
        .as_ref()
        .map(|r| r.evidence.as_slice())
        .unwrap_or(&[]);
    let platforms: Vec<_> = evidence
        .iter()
        .filter(|e| e.kind == IdentityKind::Platform && e.status == IdentityStatus::Verified)
        .filter_map(|e| e.value.as_deref())
        .filter_map(archivefs_core::canonical_platform_for_alias)
        .collect();
    let library_known = game.platform != super::library::UNKNOWN_PLATFORM;
    let conflict = platforms.iter().any(|p| {
        (library_known && *p != game.platform) || platforms.iter().any(|other| p != other)
    });
    if conflict
        || evidence.iter().any(|e| {
            e.kind == IdentityKind::Platform
                && matches!(
                    e.status,
                    IdentityStatus::Ambiguous | IdentityStatus::Invalid
                )
        })
    {
        return "Conflicting or ambiguous platform identity: review existing library evidence. Structural media inspection cannot resolve the conflict.";
    }
    match game.platform.as_str() {
        "Acorn Electron" => {
            "Library context: Acorn Electron. DFS/UEF structure remains BBC/Electron family evidence; it does not verify this platform or game."
        }
        "BBC Micro" => {
            "Library context: BBC Micro. Elkulator targets Acorn Electron; BBC identity is never converted to Electron by this preview."
        }
        _ => {
            "Platform unresolved or outside Electron: DFS/UEF structure alone cannot distinguish BBC Micro from Acorn Electron."
        }
    }
}

struct Snapshot {
    image: Arc<ElkulatorInspection>,
    discovery: ExecutableDiscovery,
    fresh: bool,
    checked_at: Instant,
}
#[derive(Default)]
struct Panel {
    job: Option<Receiver<Result<Snapshot, String>>>,
    result: Option<Snapshot>,
    error: Option<String>,
    checked: Option<Instant>,
}

fn inspect(
    source: PathBuf,
    previous: Option<Arc<ElkulatorInspection>>,
    path: Option<OsString>,
) -> Result<Snapshot, String> {
    let image = match previous {
        Some(image) => image,
        None => Arc::new(inspect_media(&source)?),
    };
    let discovery = elkulator::discover_executables(&[], path.as_deref());
    // Check again after discovery, immediately before publishing the snapshot.
    let fresh = image.is_fresh();
    Ok(Snapshot {
        image,
        discovery,
        fresh,
        checked_at: Instant::now(),
    })
}

impl Panel {
    fn start(&mut self, ctx: &egui::Context, source: PathBuf, reparse: bool) {
        if self.job.is_some() {
            return;
        }
        let previous = (!reparse)
            .then(|| self.result.as_ref().map(|result| result.image.clone()))
            .flatten();
        if reparse {
            self.result = None;
        }
        self.error = None;
        let path = std::env::var_os("PATH");
        let (tx, rx) = mpsc::channel();
        self.job = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(|| inspect(source, previous, path))
                .unwrap_or_else(|_| {
                    Err("Media inspection worker stopped; no files were changed.".into())
                });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
    fn poll(&mut self) {
        let Some(job) = &self.job else {
            return;
        };
        let result = match job.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                Err("Media inspection worker disconnected; re-inspect to continue.".into())
            }
        };
        self.job = None;
        self.checked = Some(Instant::now());
        match result {
            Ok(report) => {
                self.checked = Some(report.checked_at);
                self.result = Some(report);
            }
            Err(error) => {
                self.result = None;
                self.error = Some(error);
            }
        }
    }
}

fn render(ui: &mut egui::Ui, game: &Game, panel: &Panel) -> bool {
    let mut start = false;
    card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong("Acorn Electron / BBC media inspection");
            status_badge(ui, "Preview-only", StatusTone::Warning);
        });
        ui.label("Mr Wiz · I can check the media structure, but a recognised file is not proof of the right machine or a playable game.");
        ui.label(platform_message(game));
        ui.label("Elkulator Launch unavailable: writable-media behaviour, configuration containment and firmware provisioning are unverified. This preview never starts an emulator or prepares writable copies.");
        if ui
            .add_enabled(
                panel.job.is_none(),
                egui::Button::new("Inspect / re-inspect Acorn media"),
            )
            .clicked()
        {
            start = true;
        }
        if panel.job.is_some() {
            ui.spinner();
            ui.label("Checking source freshness in the background…");
        }
        if let Some(error) = &panel.error {
            ui.colored_label(
                theme::warning(ui),
                format!("Unsupported or unavailable media: {error}"),
            );
        }
        if let Some(report) = &panel.result {
            let status = if panel.job.is_some() {
                "Freshness check pending; previous evidence is a snapshot"
            } else if !report.fresh {
                "Stale: source changed or is missing. Re-inspect before relying on this evidence"
            } else if report.image.structure.incomplete {
                "Container inspected with incomplete or unsupported content"
            } else {
                "Structure inspected; source revalidated at the last check"
            };
            ui.label(status);
            ui.label(format!("Media type: {:?}", report.image.format));
            ui.weak("Point-in-time structural evidence only; no emulator compatibility, successful boot or current launch readiness is established.");
            if report.discovery.executables.is_empty() {
                ui.label("Elkulator not discovered on the checked PATH. Media inspection does not require an emulator.");
            } else {
                ui.label(format!(
                    "{} Elkulator executable(s) observed; none selected or executed",
                    report.discovery.executables.len()
                ));
                for path in report.discovery.executables.iter().take(8) {
                    ui.monospace(path.display().to_string());
                }
            }
            for refusal in report.discovery.symlink_refusals.iter().take(8) {
                ui.label(format!(
                    "Executable symlink refused by discovery policy: {}",
                    refusal.link.display()
                ));
            }
            ui.collapsing(
                "Structural evidence and provenance (inspection snapshot)",
                |ui| {
                    ui.label(format!(
                        "Source: {}",
                        report.image.source.original_path.display()
                    ));
                    ui.label(format!(
                        "Source bytes: {}",
                        report.image.source.original_identity.size
                    ));
                    ui.monospace(format!(
                        "SHA-256: {}",
                        report
                            .image
                            .source
                            .sha256
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>()
                    ));
                    for detail in report.image.structure.details.iter().take(64) {
                        ui.label(detail);
                    }
                    for warning in report.image.structure.warnings.iter().take(32) {
                        ui.colored_label(theme::warning(ui), warning);
                    }
                },
            );
        } else if panel.job.is_none() {
            ui.label("No media inspected yet. Standard DFS SSD/DSD and plain or gzip-wrapped UEF are inspected read-only; other media remain unsupported here.");
        }
    });
    start
}

pub(super) fn show(ui: &mut egui::Ui, game: &Game) {
    if !applies(game) {
        return;
    }
    ui.collapsing("Acorn media · Elkulator preview-only", |ui| {
        // Identity changes and another focused game never inherit old conclusions.
        let id = egui::Id::new((
            "electron-preview",
            game.archive.id,
            &game.archive.absolute_path,
            &game.platform,
            format!("{:?}", game.archive.identity_report),
        ));
        let state = ui.ctx().data_mut(|data| {
            data.get_temp_mut_or_default::<Arc<Mutex<Panel>>>(id)
                .clone()
        });
        let Ok(mut panel) = state.try_lock() else {
            ui.label("Inspection state is busy");
            return;
        };
        panel.poll();
        if panel.job.is_none()
            && panel.result.as_ref().is_some_and(|report| report.fresh)
            && panel
                .checked
                .is_some_and(|time| time.elapsed() >= RECHECK_INTERVAL)
        {
            panel.start(ui.ctx(), game.archive.absolute_path.clone(), false);
        }
        if render(ui, game, &panel) {
            panel.start(ui.ctx(), game.archive.absolute_path.clone(), true);
        }
        if panel.job.is_some() || panel.result.as_ref().is_some_and(|report| report.fresh) {
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
    });
}

#[cfg(test)]
mod tests;
