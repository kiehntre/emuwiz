//! Explicit, asynchronous read-only review; no action dispatches a publisher.
use super::*;
use archivefs_core::managed_emulator_install::review::{
    AppImageInstallReview, COMPATIBILITY, review_appimage_install,
};

impl EmulatorDownloadPageState {
    pub(super) fn start_side_by_side(
        &mut self,
        id: String,
        pick_artifact: bool,
        context: egui::Context,
    ) {
        if self.task.is_some() {
            return;
        }
        let Some(spec) = emulator_download_spec(&id)
            .filter(|s| s.distribution == EmulatorDistribution::GithubAppImage)
        else {
            return;
        };
        let artifact = if pick_artifact {
            let Some(path) = rfd::FileDialog::new()
                .set_title("Choose an already downloaded AppImage for read-only verification")
                .pick_file()
            else {
                return;
            };
            Some(path)
        } else {
            None
        };
        let key = spec.id;
        let (sender, receiver) = mpsc::channel();
        self.task = Some((id, receiver));
        self.side_by_side_busy = true;
        self.side_by_side_reviews.remove(key);
        thread::spawn(move || {
            let result = (|| {
                let root = managed_root()?;
                let transport = HttpsEmulatorDownloadTransport::new();
                let plan = resolve_download_plan(
                    &root,
                    spec,
                    &transport,
                    &EmulatorDownloadOptions::default(),
                )
                .map_err(|e| e.to_string())?;
                Ok(review_appimage_install(&root, &plan, artifact.as_deref()))
            })();
            let _ = sender.send(TaskResult::SideBySide(Box::new(result)));
            context.request_repaint();
        });
    }

    pub(super) fn show_side_by_side(
        &self,
        ui: &mut egui::Ui,
    ) -> Option<EmulatorDownloadPageAction> {
        let mut action = None;
        ui.separator();
        ui.heading("Side-by-side AppImage review");
        ui.label("Review a curated release and an optional local artifact. Review never downloads an AppImage, installs files, runs it, or changes the selected executable.");
        ui.label("These catalogue lanes are review candidates; installation remains unavailable until the publication boundary is reviewed.");
        for spec in Self::managed_specs() {
            ui.push_id(("side-by-side",spec.id),|ui| {
                ui.group(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(spec.display_name);
                        if ui.add_enabled(self.task.is_none(),egui::Button::new("Review side-by-side")).clicked() {action=Some(EmulatorDownloadPageAction::ReviewSideBySide(spec.id.into(),false));}
                        if ui.add_enabled(self.task.is_none(),egui::Button::new("Verify local AppImage…")).clicked() {action=Some(EmulatorDownloadPageAction::ReviewSideBySide(spec.id.into(),true));}
                    });
                    if self.task.as_ref().is_some_and(|(id,_)|id==spec.id) {ui.label("Checking…");}
                    if let Some(result)=self.side_by_side_reviews.get(spec.id) {
                        match result {
                            Err(error)=> {ui.label(format!("Review refused: {error}. Check the curated source or retry metadata retrieval. No artifact was downloaded."));}
                            Ok(review)=>show_review(ui,review),
                        }
                    }
                });
            });
        }
        action
    }
}
fn show_review(ui: &mut egui::Ui, review: &AppImageInstallReview) {
    let plan = &review.plan;
    widgets::status_badge(
        ui,
        if review.refusals.is_empty() {
            "Held — publication unavailable"
        } else {
            "Refused"
        },
        widgets::StatusTone::Blocked,
    );
    widgets::detail_row(ui, "Proposed release", &plan.release_tag);
    widgets::detail_row(ui, "Publisher catalogue", &plan.project_url);
    widgets::detail_row(ui, "Exact asset", &plan.asset_url);
    widgets::detail_row(
        ui,
        "Published SHA-256",
        plan.expected_sha256.as_deref().unwrap_or("missing"),
    );
    widgets::detail_row(
        ui,
        "Proposed new executable",
        &review
            .proposed_destination
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not planned until source evidence passes".into()),
    );
    if let Some(parent) = &review.staging_parent {
        widgets::detail_row(
            ui,
            "Required adjacent staging parent",
            &parent.display().to_string(),
        );
    }
    if !review.current_installations_inspected {
        ui.label("Current installations were not inspected because source or destination checks failed. Existing installations remain untouched.");
    } else if review.current_installations.is_empty() {
        ui.label("No existing managed-layout installation was observed. External/system installations and profile selections are not adopted; inspect Emulator inventory for those installations.");
    }
    for current in &review.current_installations {
        widgets::detail_row(ui, "Preserved installation", current);
    }
    if let Some(path) = &review.artifact {
        widgets::detail_row(ui, "Local artifact", &path.display().to_string());
    }
    if let Some(digest) = &review.artifact_sha256 {
        widgets::detail_row(ui, "Local bytes verified", digest);
        ui.label("The opened regular file matched the published release digest and x86_64 type 2 AppImage header at review time. This is not a permission to execute or install it.");
    }
    if let Some(free) = review.available_bytes {
        widgets::detail_row(ui, "Destination available bytes", &free.to_string());
    }
    if let Some(required) = review.required_bytes {
        widgets::detail_row(
            ui,
            "Minimum staging/publication bytes",
            &required.to_string(),
        );
    }
    ui.label(COMPATIBILITY);
    for reason in &review.refusals {
        ui.label(format!("Refused: {reason}"));
    }
    for reason in &review.pending {
        ui.label(format!("Held: {reason}"));
    }
    ui.add_enabled(
        false,
        egui::Button::new("Side-by-side installation unavailable"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn review_result_does_not_report_installation_or_modify_download_state() {
        let mut page = EmulatorDownloadPageState::default();
        page.entries.insert(
            "pcsx2",
            EmulatorDownloadEntryState::Installed("/kept/pcsx2".into()),
        );
        let before = page.entries.clone();
        page.apply_task_result(
            "pcsx2",
            TaskResult::SideBySide(Box::new(Err("missing upstream digest".into()))),
        );
        assert_eq!(page.entries, before);
        assert_eq!(page.take_completed_install(), None);
        assert_eq!(
            page.side_by_side_reviews.get("pcsx2"),
            Some(&Err("missing upstream digest".into()))
        );
    }
    #[test]
    fn blue_presents_all_curated_review_lanes_without_expanding_bootstrap_policy() {
        assert_eq!(
            EmulatorDownloadPageState::managed_specs()
                .map(|s| s.id)
                .collect::<Vec<_>>(),
            vec!["pcsx2", "ppsspp", "rpcs3", "duckstation", "xemu"]
        );
        assert_eq!(
            EmulatorDownloadPageState::managed_bootstrap_specs()
                .map(|s| s.id)
                .collect::<Vec<_>>(),
            vec!["pcsx2", "ppsspp"]
        );
    }
}
