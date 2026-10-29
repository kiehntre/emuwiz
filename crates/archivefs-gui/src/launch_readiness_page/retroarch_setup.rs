//! Selected-game explanation of the existing RetroArch discovery and mapping.
//! Suggestions here never turn into launch authority; the normal plan and
//! fresh preflight continue to decide whether Play is available.

use archivefs_core::emulator_environment::retroarch::CoreInfoFinding;
use archivefs_core::launch::{
    LaunchPlan, LaunchTarget, launch_compatibility_for_platform, retroarch_platform_matches,
};
use eframe::egui;

use super::RetroArchLaunchContext;

pub(super) fn show(
    ui: &mut egui::Ui,
    plan: &LaunchPlan,
    context: Option<&RetroArchLaunchContext>,
) -> bool {
    let Some(platform) = plan.platform_id.as_deref() else {
        return false;
    };
    if launch_compatibility_for_platform(platform).is_none() {
        return false;
    }
    let matching: Vec<_> = plan
        .candidates
        .iter()
        .filter_map(|candidate| match &candidate.target {
            LaunchTarget::RetroArchCore { core_stem, .. } => Some(core_stem.as_str()),
            _ => None,
        })
        .collect();
    if !matching.is_empty() {
        ui.small(format!("RetroArch setup: installed core {} matches {platform}. This is a launch recommendation from scanned files; Play performs a fresh safety check. Successful launch is reported separately.", matching.join(", ")));
        return false;
    }
    let mut open_setup = false;
    super::widgets::card(ui, |ui| {
        ui.strong(format!("RetroArch setup for {platform}"));
        #[cfg(not(target_os = "linux"))]
        {
            ui.label("Automatic RetroArch core setup is currently supported on Linux only.");
            return;
        }
        #[cfg(target_os = "linux")]
        {
            let Some(context) = context else {
                ui.label("RetroArch has not been checked. Open Emulator Setup and scan installations and cores.");
                open_setup = ui.button("Open Emulator Setup").clicked();
                return;
            };
            let cores: Vec<_> = context
                .environment
                .profiles
                .iter()
                .flat_map(|p| &p.cores)
                .collect();
            if context.environment.profiles.is_empty() {
                ui.label("No usable RetroArch installation was discovered. Install RetroArch through your Linux distribution or choose an existing native installation in Emulator Setup.");
            } else if cores.is_empty() {
                ui.label("RetroArch was found, but no installed cores were found in its configured core folder. Use RetroArch's own Core Downloader where available, or your distribution's libretro core packages, then scan again. Emulator Setup can point EmuWiz to an existing core folder without editing retroarch.cfg.");
            } else {
                let mut incompatible = Vec::new();
                let mut without_info = Vec::new();
                for core in cores {
                    if retroarch_platform_matches(&core.info, platform) {
                        continue;
                    }
                    if matches!(core.info, CoreInfoFinding::Found { .. }) {
                        incompatible.push(core.core_stem.as_str());
                    } else {
                        without_info.push(core.core_stem.as_str());
                    }
                }
                incompatible.sort_unstable();
                incompatible.dedup();
                without_info.sort_unstable();
                without_info.dedup();
                if !incompatible.is_empty() {
                    ui.label(format!("Installed cores for other platforms: {}. Their reviewed metadata does not identify {platform} as compatible.", incompatible.join(", ")));
                }
                if !without_info.is_empty() {
                    ui.label(format!("Core metadata is missing or unusable for: {}. Install the matching .info files before retrying; EmuWiz will not guess from filenames.", without_info.join(", ")));
                }
                ui.label(format!("Install a {platform} core using RetroArch's Core Downloader where available or your distribution's libretro packages, or select a folder already containing one. Then check again."));
            }
            ui.small("No binaries are downloaded or settings changed by this recommendation.");
            open_setup = ui.button("Open Emulator Setup").clicked();
        }
    });
    open_setup
}
