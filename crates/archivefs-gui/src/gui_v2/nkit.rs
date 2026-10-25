//! One read-only NKit header snapshot shared by Game Details and Launch.
use super::{
    library::Game,
    routes::{Route, Section},
};
use archivefs_core::gamecube_wii_boot_evidence::nkit::{
    self, NkitHeaderObservation, NkitInspectionError, NkitPlatform,
};
use eframe::egui;
use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex},
    time::SystemTime,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct SourceStamp {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}
impl SourceStamp {
    fn capture(path: &Path) -> Option<Self> {
        let metadata = fs::symlink_metadata(path).ok()?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return None;
        }
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Some(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            identity: (
                metadata.dev(),
                metadata.ino(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            ),
        })
    }
}

type Result = std::result::Result<NkitHeaderObservation, NkitInspectionError>;
#[derive(Default)]
struct Panel {
    generation: u64,
    requested: bool,
    busy: bool,
    stamp: Option<SourceStamp>,
    result: Option<Result>,
}
type SharedPanel = Arc<Mutex<Panel>>;
impl Panel {
    fn accept(
        &mut self,
        generation: u64,
        before: &Option<SourceStamp>,
        after: Option<SourceStamp>,
        result: Result,
    ) -> bool {
        if self.generation != generation {
            return false;
        }
        self.busy = false;
        if self.stamp != *before || *before != after {
            self.result = None;
            self.requested = false;
            return false;
        }
        self.result = Some(result);
        true
    }
}

fn start(
    shared: SharedPanel,
    context: egui::Context,
    path: std::path::PathBuf,
    stamp: Option<SourceStamp>,
) {
    let generation = {
        let mut panel = shared.lock().unwrap();
        if panel.busy {
            return;
        }
        panel.generation = panel.generation.wrapping_add(1);
        panel.busy = true;
        panel.requested = true;
        panel.stamp = stamp.clone();
        panel.result = None;
        panel.generation
    };
    let worker = shared.clone();
    let repaint = context.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("nkit-header-inspection".into())
        .spawn(move || {
            let result = nkit::inspect(&path);
            let after = SourceStamp::capture(&path);
            worker
                .lock()
                .unwrap()
                .accept(generation, &stamp, after, result);
            repaint.request_repaint();
        })
    {
        let mut panel = shared.lock().unwrap();
        panel.busy = false;
        panel.result = Some(Err(NkitInspectionError::UnsafeOrUnreadable(
            error.to_string(),
        )));
    }
}

fn explanation(result: &Result) -> &'static str {
    match result {
        Ok(_) => "NKit v1 header recognized · disc body unverified",
        Err(NkitInspectionError::NotNkit) => {
            "No raw NKit v1 header recognized. This does not verify a normal or complete disc; compressed inner contents were not inspected."
        }
        Err(NkitInspectionError::Truncated) => {
            "The file is too short for this NKit header inspection. Disc representation remains unverified."
        }
        Err(NkitInspectionError::Unsupported(detail)) if detail.contains("ambiguous") => {
            "Raw-disc and wrapper signatures overlap. Disc representation and NKit status remain unverified."
        }
        Err(NkitInspectionError::Unsupported(detail)) if detail.contains("wrapper") => {
            "Wrapper signature recognized · inner NKit representation uninspected. A filename cannot establish the inner format."
        }
        Err(NkitInspectionError::Unsupported(_)) => {
            "This NKit header variant is unsupported. Disc body and recoverability remain unverified."
        }
        Err(NkitInspectionError::Malformed(_)) => {
            "An NKit marker was found, but its header fields are invalid. Disc body and recoverability remain unverified."
        }
        Err(NkitInspectionError::UnsafeOrUnreadable(_)) => {
            "The selected file could not be safely inspected. Disc representation remains unverified."
        }
    }
}

pub(super) fn show(ui: &mut egui::Ui, game: &Game, launch_page: bool) -> Option<Route> {
    if !matches!(game.platform.as_str(), "GameCube" | "Wii") {
        return None;
    }
    let path = &game.archive.absolute_path;
    let id = egui::Id::new(("nkit_header", game.archive.id, path));
    let shared = ui.ctx().data_mut(|data| {
        data.get_temp::<SharedPanel>(id).unwrap_or_else(|| {
            let shared = SharedPanel::default();
            data.insert_temp(id, shared.clone());
            shared
        })
    });
    let stamp = SourceStamp::capture(path);
    let needs_inspection = {
        let mut panel = shared.lock().unwrap();
        if panel.stamp != stamp {
            panel.result = None;
        }
        !panel.busy && (!panel.requested || panel.stamp != stamp)
    };
    if needs_inspection {
        start(
            shared.clone(),
            ui.ctx().clone(),
            path.clone(),
            stamp.clone(),
        );
    }
    let mut route = None;
    let mut refresh = false;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.strong("Disc representation & preservation");
        let panel = shared.lock().unwrap();
        if panel.busy { ui.spinner(); ui.label("Inspecting the selected disc's NKit header…"); }
        if let Some(result) = &panel.result {
            ui.label(explanation(result));
            if let Ok(header) = result {
                let platform = match header.platform { NkitPlatform::GameCube => "GameCube", NkitPlatform::Wii => "Wii" };
                ui.label(format!("Header platform: {platform}. Stored size: {} bytes.", header.stored_size));
                if platform != game.platform { ui.label("The header platform differs from this library entry. Review identity before choosing another representation."); }
                ui.label("Preserve this image and any original disc or recovery data. Recoverability has not been established.");
                ui.label("Header detection does not verify the complete disc or establish an exact DAT match. The claimed original CRC and the header-prefix hash are not whole-disc verification.");
                ui.label("NKit launch is unavailable in EmuWiz. Header recognition does not establish supported Dolphin compatibility. Conversion and recovery are not available.");
                ui.collapsing("Header claims", |ui| {
                    ui.label(format!("Claimed original size: {} bytes", header.claimed_original_size));
                    ui.monospace(format!("Claimed original CRC32: {:08X}", header.claimed_original_crc32));
                    ui.label(format!("Disc number: {}; revision: {}", header.disc_number, header.revision));
                    if let Some(crc) = header.removed_update_crc32 {
                        ui.monospace(format!("Header declares removed-update span CRC32: {crc:08X}"));
                        ui.label("No recovery files were searched for or checked.");
                    }
                });
            }
        }
        refresh = ui.add_enabled(!panel.busy, egui::Button::new("Refresh NKit header inspection")).clicked();
        ui.horizontal_wrapped(|ui| {
            if ui.button("Review identity").clicked() { route = Some(Route::ReviewIdentity(game.archive.id)); }
            if ui.button("Verify stored representation").clicked() { route = Some(Route::Task { section: Section::Check, game: game.archive.id }); }
            if !launch_page && ui.button("Review Dolphin readiness").clicked() { route = Some(Route::Task { section: Section::Launch, game: game.archive.id }); }
        });
        ui.weak("Read-only header inspection. No disc conversion, recovery, full-disc scan or emulator launch is performed here.");
    });
    if refresh {
        start(shared, ui.ctx().clone(), path.clone(), stamp);
    }
    route
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_generation_or_source_replacement_cannot_publish_header_facts() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("disc.iso");
        fs::write(&path, vec![0u8; 0x2440]).unwrap();
        let old = SourceStamp::capture(&path);
        let mut panel = Panel {
            generation: 2,
            stamp: old.clone(),
            requested: true,
            busy: true,
            result: None,
        };
        assert!(!panel.accept(1, &old, old.clone(), Err(NkitInspectionError::NotNkit)));
        assert!(panel.busy);
        fs::write(&path, vec![0u8; 0x50000]).unwrap();
        let replacement = SourceStamp::capture(&path);
        assert_ne!(old, replacement);
        assert!(!panel.accept(2, &old, replacement, Err(NkitInspectionError::NotNkit)));
        assert!(panel.result.is_none());
        assert!(!panel.requested);
        assert!(!panel.busy);
    }

    #[test]
    fn unknown_wrapped_missing_and_invalid_content_never_claim_normal_disc_verification() {
        let errors = [
            NkitInspectionError::NotNkit,
            NkitInspectionError::Truncated,
            NkitInspectionError::Unsupported("GCZ wrapper; inner NKit variant not inspected"),
            NkitInspectionError::Unsupported("NKit version"),
            NkitInspectionError::Malformed("length"),
            NkitInspectionError::UnsafeOrUnreadable("missing".into()),
        ];
        for error in errors {
            let text = explanation(&Err(error));
            assert!(
                text.contains("unverified")
                    || text.contains("uninspected")
                    || text.contains("does not verify")
            );
            assert!(!text.contains("Verified disc"));
        }
    }
}

impl super::App {
    pub(super) fn nkit_guidance(&mut self, ui: &mut egui::Ui, game: &Game, launch_page: bool) {
        if let Some(route) = show(ui, game, launch_page) {
            if matches!(
                route,
                Route::Task {
                    section: Section::Check,
                    ..
                }
            ) {
                self.check_platform = Some(game.platform.clone());
            }
            self.go(route);
        }
    }
}
