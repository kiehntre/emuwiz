use super::*;
use crate::ui::{components as w, theme};
fn label(v: &Value) -> &str {
    v.as_str().unwrap_or("unavailable")
}
fn button(ui: &mut egui::Ui, text: &str, enabled: bool) -> bool {
    w::action_button(ui, text, w::ActionStyle::Secondary, enabled).clicked()
}
pub(super) fn show(
    s: &mut State,
    ui: &mut egui::Ui,
    input: Result<Selection, String>,
    history: bool,
) {
    w::workflow_card(ui, theme::accent(ui), |ui| {
        egui::ScrollArea::vertical().id_salt(if history{"r2-history-scroll"}else{"r2-operation-scroll"}).max_height(ui.available_height().max(180.0).min(440.0)).show(ui,|ui| {
        ui.heading(if history {
            "Advanced Protection · History & Undo"
        } else {
            "Advanced Protection · Rename/Move"
        });
        w::workflow_strip(
            ui,
            &["Preview", "Confirm", "Apply", "History", "Undo"],
            if s.review.is_some() {
                1
            } else if s
                .info
                .as_ref()
                .is_some_and(|i| i.status["state"] == "applied")
            {
                3
            } else {
                0
            },
        );
        ui.label("Optional extra identity and transaction-specific recovery checks. Standard Rename is recommended for everyday use.");
        super::super::guidance::show_tip(ui,&super::super::guidance::GuidanceTip{
          key:"advanced_protection.explain",category:super::super::guidance::GuidanceCategory::Explain,
          mascot:super::super::guidance::MascotState::Helpful,message:copy::MR_WIZ.into()});
        ui.label(copy::SCOPE);
        ui.label(copy::LIMITS);
        ui.collapsing("Measured timing examples",|ui|{ui.label(copy::BENCHMARKS);});
        ui.label("Keep the safety workspace: it contains the recovery evidence needed after interruption. Standard Rename remains the default. It is never selected automatically after a refusal.");
        let busy = s.busy();
        ui.horizontal_wrapped(|ui| {
            if button(ui, "Use selected preview's folder", !busy && !history && s.can_choose_standard()) {
                match &input {
                    Ok(i) => match i.pairs() {
                        Ok((r, _)) => s.select_root(r),
                        Err(e) => s.message = e,
                    },
                    Err(e) => s.message = e.clone(),
                }
            }
            if button(ui, "Choose collection folder…", !busy && s.can_choose_standard()) {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    s.select_root(p);
                }
            }
        });
        if let Some(root) = &s.root {
            ui.label(format!("Collection: {}", root.display()));
        }
        let helper = match Helper::bundled() {
            Ok(h) => h,
            Err(e) => {
                ui.label(e);
                return;
            }
        };
        if s.root.is_some() && s.info.is_none() {
            let root = s.root.clone().unwrap();
            ui.horizontal_wrapped(|ui| {
                if button(ui, "Check compatibility", !busy) {
                    let h = helper.clone();
                    let r = root.clone();
                    s.start(ui.ctx(), move || {
                        let v = h.run(vec![
                            "blue-eligibility".into(),
                            r.to_string_lossy().into_owned(),
                        ])?;
                        Ok(ResultValue::Eligibility(r, v))
                    });
                }
                if button(ui, "Open existing safety history", !busy) {
                    let h = helper.clone();
                    let r = root.clone();
                    s.start(ui.ctx(), move || {
                        let ws = backend::workspace(&r)?;
                        Ok(ResultValue::Info(Info::load(&h, &r, &ws, false)?))
                    });
                }
            });
            if s.eligibility.is_some() {
                ui.label("Folder profile passed. Setup will run the required filesystem capability probes; nothing is enrolled or renamed by setup.");
                ui.add_enabled(!busy,egui::Checkbox::new(&mut s.setup_confirm,"I choose optional Advanced Protection for this folder and accept the limits above."));
                if button(ui, "Prepare safety workspace", !busy && s.setup_confirm) {
                    let h = helper.clone();
                    let r = root.clone();
                    s.start(ui.ctx(), move || {
                        let ws = backend::workspace(&r)?;
                        Ok(ResultValue::Info(Info::load(&h, &r, &ws, true)?))
                    });
                }
            }
        }
        if let Some(info) = s.info.clone() {
            w::status_badge(
                ui,
                format!(
                    "{} · ownership {}",
                    state_label(&info.status["state"]),
                    label(&info.status["ownership"])
                ),
                if info.status["state"] == "review_required" {
                    w::StatusTone::Blocked
                } else {
                    w::StatusTone::Info
                },
            );
            if let Some(reasons) = info.status["reasons"].as_array() {
                for reason in reasons {
                    ui.label(label(reason));
                }
            }
            if button(ui, "Refresh current state", !busy) {
                let h = helper.clone();
                let i = info.clone();
                s.start(ui.ctx(), move || Ok(ResultValue::Info(i.refresh(&h)?)));
            }
            if s.needs_refresh {
                ui.label("Last verified snapshot only. Refresh current state before any further action.");
                ui.label(&s.message);
                return;
            }
            if !history && info.status["state"] == "not_enrolled" {
                match &input {
                    Ok(i) => {
                        if let Err(e) = i.pairs() {
                            ui.label(e);
                        } else if button(ui, "Preview selected files with Advanced Protection", !busy) {
                            let h = helper.clone();
                            let x = i.clone();
                            let inf = info.clone();
                            s.start(ui.ctx(), move || {
                                Ok(ResultValue::Preview(backend::prepare(&h, &inf, x)?))
                            });
                        }
                    }
                    Err(e) => {
                        ui.label(e);
                    }
                }
            }
            if let Some(p) = s.preview.clone() {
                ui.separator();
                ui.strong("Review exact file changes");
                show_entries(ui, &p.plan);
                ui.add_enabled(
                    !busy,
                    egui::Checkbox::new(
                        &mut s.preview_confirm,
                        "These are the exact files and names I want to apply.",
                    ),
                );
                if button(
                    ui,
                    "Confirm preview and prepare Apply review",
                    !busy && s.preview_confirm,
                ) {
                    let h = helper.clone();
                    let inf = info.clone();
                    s.preview = None;
                    s.start(ui.ctx(), move || {
                        Ok(ResultValue::Review(backend::enroll(&h, &inf, &p)?,inf.refresh(&h)?))
                    });
                }
            }
            if info.status["state"]=="awaiting_apply" && s.review.is_none() {
                ui.label("This transaction is enrolled but not applied. Review the matching selection in Organisation, or choose fresh Undo to abandon it safely.");
                if let Ok(selection)=&input {if button(ui,"Review registered Apply against selected preview",!busy){let h=helper.clone();let inf=info.clone();let selected=selection.clone();s.start(ui.ctx(),move||Ok(ResultValue::Review(backend::review_apply(&h,&inf,selected)?,inf.refresh(&h)?)));}}
            }
            ui.horizontal_wrapped(|ui| {
                // Review is participating. It is never triggered by painting/refresh.
                if button(ui, "Review fresh Undo / interrupted recovery", !busy) {
                    let h = helper.clone();
                    let inf = info.clone();
                    s.review = None;
                    s.start(ui.ctx(), move || {
                        Ok(ResultValue::Review(backend::review(&h,&inf,"undo",None)?,inf.refresh(&h)?))
                    });
                }
                if button(ui, "Review retaining changed files separately", !busy) {
                    let h = helper.clone();
                    let inf = info.clone();
                    s.review = None;
                    s.start(ui.ctx(), move || {
                        Ok(ResultValue::Review(backend::review(&h,&inf,"retain-foreign",None)?,inf.refresh(&h)?))
                    });
                }
            });
            ui.small("Recovery review may finish interrupted bookkeeping. It never approves or moves files by itself. Unknown or contradictory files stay held for review.");
            if let Some(r) = s.review.clone() {
                ui.separator();
                ui.strong(match r.action.as_str() {
                    "apply" => "Confirm Apply",
                    "undo" => "Confirm freshly reviewed Undo",
                    _ => "Separate decision: retain changed files safely",
                });
                show_entries(ui, &r.value["transaction"]);
                ui.collapsing("Exact original-file identity evidence", |ui| { evidence(ui,&r.value["transaction"]); });
                if r.action == "retain-foreign" {
                    ui.strong("Review the foreign held objects and preserved original anchors");
                    ui.label(format!("Interrupted checkpoint pending: {}", r.value["checkpoint_pending"]));
                    evidence(ui,&r.value["holding"]);
                }
                ui.label(label(&r.value["operator_effect"]));
                ui.label(format!("Eligible entries: {}", r.value["eligible_entries"]));
                let actionable = r.value["actionable"] == true;
                if !actionable {
                    ui.label(r.value["refusal"].as_str().unwrap_or(
                        "No supported action; preserve evidence and seek operator review.",
                    ));
                }
                if r.action == "retain-foreign" {
                    ui.label("This keeps reviewed changed files in holding; it does not discard or return them. Afterwards, review a fresh Undo to restore eligible originals and verify ownership release.");
                }
                ui.add_enabled(
                    !busy && actionable,
                    egui::Checkbox::new(
                        &mut s.action_confirm,
                        "I approve this exact reviewed action and its effect.",
                    ),
                );
                if button(
                    ui,
                    match r.action.as_str() {
                        "apply" => "Approve and Apply",
                        "undo" => "Approve fresh Undo",
                        _ => "Approve separate retain-files decision",
                    },
                    !busy && actionable && s.action_confirm,
                ) {
                    let h = helper.clone();
                    let inf = info.clone();
                    s.review = None;
                    s.start(ui.ctx(), move || {
                        let (v, i) = backend::execute(&h, &inf, &r)?;
                        Ok(ResultValue::Executed(v, i))
                    });
                }
            }
            if info.status["state"] == "settled" && info.status["ownership"] == "released" {
                if button(ui, "Review another transaction on this folder", !busy) {
                    let h = helper.clone();
                    let i = info.clone();
                    s.start(ui.ctx(), move || {
                        Ok(ResultValue::Reuse(backend::reuse_preview(&h, &i)?))
                    });
                }
            }
            if let Some(r) = s.reuse.clone() {
                ui.label(label(&r["effect"]));
                ui.add_enabled(
                    !busy,
                    egui::Checkbox::new(
                        &mut s.reuse_confirm,
                        "Start a new transaction while preserving all previous recovery evidence.",
                    ),
                );
                if button(ui, "Confirm new transaction", !busy && s.reuse_confirm) {
                    let h = helper.clone();
                    let i = info.clone();
                    s.start(ui.ctx(), move || {
                        Ok(ResultValue::Info(backend::reuse(&h, &i, &r)?))
                    });
                }
            }
            if let Some(entries) = info.status["entries"].as_array() {
                for e in entries {
                    ui.label(format!("File {}: {}", e["index"], label(&e["verdict"])));
                    if verified_inventory(&info.status) && !e["retained_foreign"].is_null() {
                        let retained = &e["retained_foreign"];
                        ui.label(format!(
                            "Retained copy verified: {}",
                            label(&retained["path"])
                        ));
                        ui.monospace(label(&retained["sha256"]));
                        if button(
                            ui,
                            &format!("Retrieve retained file {} to a new filename…", e["index"]),
                            !busy,
                        ) {
                            if let Some(dest) = rfd::FileDialog::new()
                                .set_file_name("retained-file-copy")
                                .save_file()
                            {
                                let h = helper.clone();
                                let i = info.clone();
                                let r = retained.clone();
                                let index = e["index"].as_u64().unwrap_or(u64::MAX) as usize;
                                s.start(ui.ctx(), move || {
                                    Ok(ResultValue::Retrieved(backend::retrieve(
                                        &h, &i, index, &r, &dest,
                                    )?))
                                });
                            }
                        }
                    }
                }
            }
            if button(ui, "Load verified earlier transactions", !busy) {
                let h = helper.clone();
                let i = info.clone();
                s.start(ui.ctx(), move || {
                    Ok(ResultValue::History(backend::history(&h, &i)?))
                });
            }
            ui.small("Verifying earlier transactions may finish interrupted admission bookkeeping. It does not rename files.");
            if let Some(v) = s.history.clone() {
                if let Some(rows) = v.as_array() {
                    for row in rows {
                        ui.label(format!(
                            "{} · verified historical settlement · retained files {}",
                            label(&row["transaction"]["transaction_id"]),
                            row["retained"].as_array().map_or(0, Vec::len)
                        ));
                        if let Some(retained) = row["retained"].as_array() { for (n, receipt) in retained.iter().enumerate() {
                            ui.label(format!("Preserved retained file: {}",label(&receipt["path"])));
                            ui.monospace(label(&receipt["sha256"]));
                            if button(ui,&format!("Retrieve historical retained copy {}…",n+1),!busy) {
                                if let Some(dest)=rfd::FileDialog::new().set_file_name("retained-file-copy").save_file() {
                                    let h=helper.clone();let mut i=info.clone();
                                    if let Some(state)=row["state"].as_str() { i.state=state.into();
                                        let receipt=receipt.clone();s.start(ui.ctx(),move||Ok(ResultValue::Retrieved(backend::retrieve_historical(&h,&i,&receipt,&dest)?)));
                                    }
                                }
                            }
                        }}
                    }
                }
            }
            ui.collapsing("Recovery evidence location",|ui|{ui.label(info.workspace.display().to_string());ui.label("Keep this directory and all holding files. Do not restore older copies over live state.");});
        }
        if let Some(v) = &s.result {
            if v["completed"] == true {
                ui.label("Operation completed. The current state above records whether ownership is released.");
            } else if v["copied"] == true {
                ui.label(format!(
                    "Retrieved verified copy: {}",
                    label(&v["destination"])
                ));
            } else {
                ui.label("Operation did not report complete Undo. A retain-files decision may have succeeded; inspect current per-file evidence and prepare a separate fresh Undo. Nothing is automatically retried.");
            }
        }
        if !s.message.is_empty() {
            w::banner(
                ui,
                "Advanced Protection needs attention",
                &s.message,
                w::StatusTone::Blocked,
            );
        }
        if busy {
            ui.spinner();
            ui.label("Checking or processing this explicitly selected operation… Other pages remain available.");
        }
    });
    });
}
fn show_entries(ui: &mut egui::Ui, tx: &Value) {
    if let Some(entries) = tx["entries"].as_array() {
        for e in entries.iter().take(8) {
            ui.horizontal_wrapped(|ui| {
                ui.label(label(&e["source_path"]));
                ui.label("→");
                ui.strong(label(&e["destination_path"]));
            });
        }
        ui.label(format!("{} file(s)", entries.len()));
    }
}

pub(super) fn verified_inventory(status: &Value) -> bool {
    matches!(
        status["state"].as_str(),
        Some(
            "applied"
                | "apply_incomplete"
                | "undo_incomplete"
                | "settled"
                | "settled_awaiting_release"
        )
    )
}

fn evidence(ui: &mut egui::Ui, value: &Value) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(serde_json::to_string_pretty(value).unwrap_or_default())
                .monospace(),
        )
        .wrap(),
    );
}
fn state_label(value: &Value) -> &str {
    match value.as_str() {
        Some("not_enrolled") => "Ready for a preview",
        Some("awaiting_apply") => "Waiting for reviewed Apply",
        Some("apply_incomplete") => "Apply interrupted",
        Some("applied") => "Applied",
        Some("undo_incomplete") => "Undo interrupted",
        Some("settled_awaiting_release") => "Settled; ownership release pending",
        Some("settled") => "Settled",
        Some("review_required") => "Review needed",
        Some("busy") => "Another operation is running",
        _ => "State unavailable",
    }
}
