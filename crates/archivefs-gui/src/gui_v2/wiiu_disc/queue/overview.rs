//! Shared Converter and read-only History views of the existing durable queue.
use super::*;

fn plan_of(reviewed: &ReviewedConversion) -> &WiiUConversionPlan {
    match reviewed {
        ReviewedConversion::WudToWux(p) | ReviewedConversion::WuxToWud(p) => p,
    }
}

pub(super) fn space_details(ui: &mut egui::Ui, plan: &WiiUConversionPlan) {
    let optional = |n: Option<u64>| n.map_or_else(|| "Unknown".into(), human_bytes);
    ui.label(format!(
        "Format: {} → {}",
        format!("{:?}", plan.source_format).to_ascii_uppercase(),
        format!("{:?}", plan.target_format).to_ascii_uppercase()
    ));
    ui.label(format!(
        "Original size: {}",
        human_bytes(plan.space.source_bytes)
    ));
    let output = if let Some(n) = plan.space.destination_exact_bytes {
        human_bytes(n)
    } else if let Some(n) = plan.space.destination_maximum_bytes {
        format!("up to {}", human_bytes(n))
    } else {
        "Unknown".into()
    };
    ui.label(format!("New file size: {output}"));
    ui.label(format!(
        "Staging space: {}",
        optional(plan.space.temporary_bytes)
    ));
    ui.label(format!(
        "Free at review: {}",
        optional(plan.space.available_bytes)
    ));
    ui.small(
        "Original kept. Source, destination and free space are checked again before execution.",
    );
    if plan.space.destination_exact_bytes.is_none() {
        ui.small("Actual output size and any space saving are unknown until conversion finishes.");
    }
}

fn status(c: &mut Controller, ui: &mut egui::Ui) -> Option<QueueSnapshot> {
    if let Some(notice) = c.notice.clone() {
        ui.colored_label(ui.visuals().warn_fg_color, notice);
    }
    match &c.snapshot {
        Some(Ok(snapshot)) => Some(snapshot.clone()),
        Some(Err(error)) => {
            ui.label("The conversion queue could not be read. Its files have been left in place.");
            ui.collapsing("Queue error details", |ui| {
                ui.label(error);
            });
            None
        }
        None => {
            ui.spinner();
            ui.label("Reading the conversion queue…");
            None
        }
    }
}

pub(super) fn show(ui: &mut egui::Ui, c: &mut Controller) -> bool {
    let ctx = ui.ctx().clone();
    c.prepare_view(&ctx, true);
    ui.heading("Pending conversions");
    ui.label("The durable queue supports WUD ↔ WUX. Add reviewed items, then explicitly start the waiting conversions. Other formats use the supported workflows below.");
    let mut history = false;
    ui.horizontal_wrapped(|ui| {
        history = ui.button("Open conversion records in History").clicked();
        if ui.button("Choose WUD or WUX…").clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Wii U disc", &["wud", "wux"])
                .pick_file()
        {
            let direction = match path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("wud") => Some(WiiUConversionDirection::WudToWux),
                Some("wux") => Some(WiiUConversionDirection::WuxToWud),
                _ => None,
            };
            if let Some(direction) = direction {
                c.selected = Some((path.clone(), direction));
                c.request_plan(&ctx, &path, direction);
            } else {
                c.notice = Some("Choose a supported WUD or WUX source. Nothing was queued.".into());
            }
        }
        if ui.button("Refresh queue").clicked() {
            c.request_snapshot(&ctx);
            c.send(&ctx, Cmd::Recover);
        }
    });
    // The existing per-source review and confirmation, without its compact list.
    if let Some((path, direction)) = c.selected.clone() {
        ui.group(|ui| {
            ui.strong(path.file_name().unwrap_or_default().to_string_lossy());
            show_path_with(ui, &path, direction, c, false);
        });
        if ui.button("Close source preview").clicked() {
            c.selected = None;
            c.confirm = None;
        }
    }
    let snapshot = status(c, ui);
    let mut action = None;
    if let Some(snapshot) = &snapshot {
        if snapshot.jobs.is_empty() {
            ui.label("No pending or retained conversion records.");
        }
        let mut ordered: Vec<_> = snapshot.jobs.iter().collect();
        ordered.sort_by_key(|j| j.order);
        if ordered.iter().any(|j| j.state == JobState::Queued)
            && queue_button(ui, "Start queued conversions", c.locked)
        {
            action = Some(Action::Cmd(Cmd::Start));
        }
        ui.small("Start includes the items waiting now. Later additions or retries wait for another Start. A failed, cancelled or interrupted item pauses the remaining queue.");
        egui::ScrollArea::vertical()
            .id_salt("converter-durable-items")
            .max_height(380.0)
            .show(ui, |ui| {
                for job in ordered {
                    ui.push_id(("converter-job", job.id), |ui| {
                        ui.group(|ui| {
                            ui.strong(
                                job.reviewed
                                    .source()
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy(),
                            );
                            space_details(ui, plan_of(&job.reviewed));
                            let live = c.progress.get(&job.id).or(job.progress.as_ref());
                            job_card(ui, job, live, c, &mut action);
                            record_details(ui, job);
                        });
                    });
                }
            });
    }
    if let Some(action) = action {
        dispatch(c, &ctx, action);
    }
    history
}

fn dispatch(c: &mut Controller, ctx: &egui::Context, action: Action) {
    match action {
        Action::Cmd(cmd) => c.send(ctx, cmd),
        Action::CancelRunning(id) => c.cancel_running(id),
        _ => {}
    }
}

fn record_details(ui: &mut egui::Ui, job: &ConversionJob) {
    ui.collapsing("Paths and recorded verification", |ui| {
        ui.label(format!("Source: {}", job.reviewed.source().display()));
        ui.label(format!("Destination: {}", job.reviewed.destination().display()));
        ui.label(format!("Planned verification: {}", plan_of(&job.reviewed).verification.steps.join(" → ")));
        if let Some(record) = &job.result {
            ui.label(format!("Recorded output size: {}", human_bytes(record.output_bytes)));
            ui.label(format!("Repair transaction: {}", record.transaction_id));
            ui.label(format!("Recorded logical-stream SHA-256: {}", record.reconstructed_wud_sha256));
            ui.label(format!("Recorded output SHA-256: {}", record.output_sha256));
            ui.small("Recorded conversion verification is not a current file check or a DAT audit.");
        } else { ui.label("No successful conversion result is recorded."); }
        for (n, attempt) in job.attempts.iter().enumerate() {
            ui.label(format!("Attempt {}: {:?}", n + 1, attempt.state));
            ui.label(format!("Repair journals: {}", attempt.journal_dir.display()));
            ui.label(format!("Staging evidence: {}", attempt.staging_root.display()));
        }
        ui.small("Keep journals and staging evidence for review. This view does not authorise Undo, deletion or recovery.");
    });
}

/// History only inspects; it never acquires the writer lock or recovers state.
pub(super) fn history(ui: &mut egui::Ui, c: &mut Controller, source: Option<&Path>) -> bool {
    let ctx = ui.ctx().clone();
    c.prepare_view(&ctx, false);
    ui.heading("Conversion records");
    ui.small("Retained queue entries only. A recorded result describes that attempt, not the current file state. Entries hidden from the queue are absent here; their Repair journals are kept.");
    if ui.button("Refresh conversion records").clicked() {
        c.request_snapshot(&ctx);
    }
    let Some(snapshot) = status(c, ui) else {
        return false;
    };
    let jobs: Vec<_> = snapshot
        .jobs
        .iter()
        .filter(|j| source.is_none_or(|p| j.reviewed.source() == p))
        .collect();
    let has_records = !jobs.is_empty();
    if !has_records {
        ui.label("No retained conversion records for this selection.");
    }
    egui::ScrollArea::vertical().id_salt("conversion-history-items").max_height(260.0).show(ui, |ui| {
        for job in jobs {
            ui.push_id(("conversion-history", job.id), |ui| {
                ui.group(|ui| {
                    ui.strong(job.reviewed.source().file_name().unwrap_or_default().to_string_lossy());
                    let (badge, meaning) = job_text(job.state, job.retry);
                    if job.state == JobState::Running {
                        ui.label("Last recorded state: running. Inspect Converter for the current outcome; this view does not recover or restart it.");
                    } else { ui.label(format!("{badge}: {meaning}")); }
                    if let Some(reason) = &job.diagnostic { ui.label(reason); }
                    record_details(ui, job);
                });
            });
        }
    });
    ui.small(
        "Open Converter to review pending or interrupted items. Undo is not offered by this view.",
    );
    has_records
}
