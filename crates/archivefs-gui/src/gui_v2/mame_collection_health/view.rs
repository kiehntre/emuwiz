//! Beginner-facing read-only projection of actual analyser facts.
use crate::dat_catalogue_picker::DatCatalogueWorkflow;
use crate::ui::{components as widgets, theme};
use archivefs_core::mame_collection_analyser::{
    MameCollectionStyle, MameMemberEvidence, MamePreservationStatus, MameRunnability,
    MameSetHealth, MameSetResult, MameVersionCompatibility,
};
use eframe::egui;

use super::{
    page::{Filter, HealthPage},
    worker::Report,
};

const PAGE_SIZE: usize = 50;

pub(super) fn text(value: &str) -> String {
    let shortened: String = value.chars().take(512).collect();
    let mut safe = super::super::identity_review::provenance_text(&shortened);
    if value.chars().count() > 512 {
        safe.push_str("… (full value in report)");
    }
    safe
}

pub(super) fn show(ui: &mut egui::Ui, page: &mut HealthPage) {
    ui.heading("MAME Collection Health");
    ui.label("Inspect your arcade sets without changing them. Good means verified storage at the time of inspection; it does not prove game runnability.");
    let busy = page.job.is_some();
    ui.add_enabled_ui(!busy, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label("Collection folder");
            if ui.add(egui::TextEdit::singleline(&mut page.root).desired_width(ui.available_width().min(300.0)).hint_text("Choose a folder containing ZIP sets or machine folders")).changed() { page.invalidate_inputs(); }
            if ui.button("Choose folder…").clicked() && let Some(path) = rfd::FileDialog::new().pick_folder() { page.root = path.to_string_lossy().into(); page.invalidate_inputs(); }
        });
        if let Some(reference) = &page.selected { ui.label(format!("Selected catalogue: {}", text(&reference.token()))); }
        else { ui.label("No MAME arcade catalogue selected."); }
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(page.selected.is_some() && !page.root.trim().is_empty(), egui::Button::new("Inspect collection").fill(theme::primary_action_fill(ui))).clicked() { page.start(); }
            if ui.button("Refresh catalogue list").clicked() { page.picker.refresh(); page.selected = None; page.invalidate_inputs(); }
        });
        egui::CollapsingHeader::new("Choose MAME arcade catalogue").default_open(page.selected.is_none()).show(ui, |ui| {
            ui.label("Use your installed MAME arcade machine catalogue. Software lists are excluded. Add an existing MAME -listxml file in Verification Data if none is listed; this page never downloads or updates it.");
            let previous = page.selected.clone();
            let _ = page.picker.show(ui, DatCatalogueWorkflow::MameHealth, &mut page.selected);
            if previous != page.selected { page.invalidate_inputs(); }
        });
        ui.collapsing("Installed MAME version evidence (optional)", |ui| {
            ui.label("Paste captured MAME -version output to compare releases. It is user-supplied evidence; this page does not execute an emulator or infer a version from set counts.");
            if ui.add(egui::TextEdit::singleline(&mut page.version_output).char_limit(4096).hint_text("For example: MAME 0.280").desired_width(ui.available_width())).changed() { page.invalidate_inputs(); }
        });
    });
    if let Some(job) = &page.job {
        ui.horizontal_wrapped(|ui| {
            ui.spinner();
            ui.label(if job.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                "Cancelling; waiting for the current bounded step to stop…"
            } else {
                "Read-only inspection is running…"
            });
            if ui.button("Cancel inspection").clicked() {
                job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        });
        if let Some(p) = &page.progress {
            ui.label(format!(
                "{} entries listed · {} containers collected · {:.1} MiB logical read budget used",
                p.entries,
                p.containers,
                p.logical_bytes as f64 / 1048576.0
            ));
            ui.label(format!("Now: {}", text(&p.current.to_string_lossy())));
        }
    }
    if let Some(message) = &page.message {
        ui.label(text(message));
    }
    ui.collapsing("What do these results mean?", result_explanations);
    if page.checking_sources() {
        ui.horizontal_wrapped(|ui| {
            ui.spinner();
            ui.label("Checking snapshot source identities before showing results…");
        });
        return;
    }
    let Some(report) = &page.report else {
        if !busy {
            ui.strong("No inspection report yet");
            ui.label("Choose a catalogue and a folder, then select Inspect collection. Filenames and ZIP CRC metadata alone cannot prove verified content.");
        }
        return;
    };
    summary(ui, report);
    ui.horizontal_wrapped(|ui| {
        if ui.button("Copy report summary").clicked() {
            ui.ctx().copy_text(summary_text(report));
        }
        if ui
            .add_enabled(
                !report.json.is_empty(),
                egui::Button::new("Copy JSON report"),
            )
            .clicked()
        {
            ui.ctx().copy_text(report.json.clone());
        }
    });
    if let Some(reason) = &report.json_unavailable {
        ui.label(text(reason));
    }
    ui.label("Reports are snapshots, not live monitoring. Reinspect after any collection or catalogue change. Clipboard export changes no collection, catalogue, database or journal.");
    if !report.analysis.warnings.is_empty() {
        ui.collapsing(
            format!("Inspection notes ({})", report.analysis.warnings.len()),
            |ui| {
                for w in report.analysis.warnings.iter().take(30) {
                    ui.label(text(w));
                }
            },
        );
    }
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        if ui
            .add(
                egui::TextEdit::singleline(&mut page.query)
                    .hint_text("Find set or description")
                    .desired_width(ui.available_width().min(280.0)),
            )
            .changed()
        {
            page.offset = 0;
        }
        for (filter, label) in [
            (Filter::All, "All"),
            (Filter::Good, "Good"),
            (Filter::Bad, "Bad"),
            (Filter::Unknown, "Unknown"),
        ] {
            if ui
                .selectable_value(&mut page.filter, filter, label)
                .changed()
            {
                page.offset = 0;
            }
        }
    });
    let query = page.query.trim().to_lowercase();
    let matching: Vec<_> = report
        .analysis
        .set_results
        .iter()
        .map(|s| (&s.name, Some(s), s.health))
        .chain(
            report
                .unrecognised_sets
                .iter()
                .map(|name| (name, None, MameSetHealth::Unknown)),
        )
        .filter(|(name, _, health)| {
            let status = match page.filter {
                Filter::All => true,
                Filter::Good => *health == MameSetHealth::Good,
                Filter::Bad => *health == MameSetHealth::Bad,
                Filter::Unknown => *health == MameSetHealth::Unknown,
            };
            status
                && (query.is_empty()
                    || name.to_lowercase().contains(&query)
                    || report
                        .context
                        .get(*name)
                        .and_then(|s| s.description.as_ref())
                        .is_some_and(|s| s.to_lowercase().contains(&query)))
        })
        .collect();
    page.offset = page
        .offset
        .min(matching.len().saturating_sub(1) / PAGE_SIZE * PAGE_SIZE);
    ui.label(format!("{} matching set names. Unowned catalogue sets can be Unknown; the catalogue is not a list of games you must acquire. Unknown counts also include unrecognised collection containers, grouped here by name.", matching.len()));
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(page.offset > 0, egui::Button::new("Previous sets"))
            .clicked()
        {
            page.offset = page.offset.saturating_sub(PAGE_SIZE);
        }
        if ui
            .add_enabled(
                page.offset + PAGE_SIZE < matching.len(),
                egui::Button::new("Next sets"),
            )
            .clicked()
        {
            page.offset += PAGE_SIZE;
        }
    });
    for (name, set, status) in matching.iter().skip(page.offset).take(PAGE_SIZE) {
        ui.push_id(name, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong(text(name));
                ui.label(health(*status));
                if let Some(set) = set {
                    ui.label(format!(
                        "{} / {} ROM or disk requirements verified",
                        set.matched_members, set.expected_members
                    ));
                } else {
                    ui.label("No matching machine declaration");
                }
                if ui.button("Why?").clicked() {
                    page.selected_set = Some((*name).clone());
                }
            });
        });
    }
    if let Some(name) = &page.selected_set {
        if let Some(set) = report.analysis.set_results.iter().find(|s| &s.name == name) {
            if let Some(copy_name) = why(ui, report, set) {
                page.copy_set(copy_name);
            }
        } else if report.unrecognised_sets.contains(name) {
            widgets::card(ui, |ui| {
                ui.heading(format!("Why? {}", text(name)));
                ui.strong(health(MameSetHealth::Unknown));
                ui.label("No matching machine declaration exists in the selected arcade catalogue. Observed filenames, sizes and hashes cannot establish verified content without catalogue requirements.");
                for container in report
                    .inspection
                    .inventory
                    .sets
                    .iter()
                    .filter(|s| &s.name == name)
                    .take(10)
                {
                    ui.label(format!(
                        "Observed container: {} · {} members",
                        text(&container.location.to_string_lossy()),
                        container.members.len()
                    ));
                }
                ui.label("No game runnability was established. Full report copying retains the inspected container evidence and source identity.");
            });
        }
    }
}

fn result_explanations(ui: &mut egui::Ui) {
    ui.label("Good: required content was verified against the catalogue in a complete inspection.");
    ui.label("Bad: required content is missing or disagrees with the catalogue. Open Why? to see which ROM or disk is affected.");
    ui.label("Unknown: the evidence cannot establish completeness. The inspection may be incomplete, a checksum unavailable, or a dependency unresolved.");
    ui.label("Missing ROMs: required files were not found. A clone may need files from a parent, BIOS or device set. A matching filename alone does not verify content.");
    ui.label("Incorrect hashes: a checksum or size disagrees with the catalogue, so the inspected bytes do not match the expected content.");
    // Reuse the distinctions explained by Mr Wiz's mame.bad_dump_reference
    // and mame.no_dump_reference guidance without inventing assessment facts.
    ui.label("BAD_DUMP: the reference marks this as a known imperfect dump. That alone does not mean your copy has become damaged. Even a matching copy retains this preservation limitation.");
    ui.label("NO_DUMP: no verified good dump is known for this member. This records a preservation gap; it does not establish that a usable replacement exists.");
    ui.label("CHDs: Missing means the required disk image was not found. Unverified means its content and any parent chain have not been verified; a .chd filename or its presence is insufficient.");
    ui.label("Choose a set's Why? for the evidence behind its result. Complete storage does not prove game runnability; this page does not test whether games run.");
}

fn summary(ui: &mut egui::Ui, report: &Report) {
    ui.separator();
    ui.strong(if report.inspection.inventory.inspected_completely {
        "ROM inspection complete within the stated scope"
    } else {
        "Incomplete inspection — no verified-complete sets"
    });
    ui.label(format!(
        "Catalogue: {}",
        text(&report.catalogue.display_name)
    ));
    if let Some(author) = report
        .catalogue
        .recorded_author
        .as_deref()
        .filter(|a| !a.trim().is_empty())
    {
        ui.label(format!("Recorded author: {}", text(author)));
    }
    ui.label(format!(
        "Catalogue version: {}",
        text(
            report
                .analysis
                .catalogue_version
                .as_deref()
                .unwrap_or("Unknown")
        )
    ));
    ui.label(format!(
        "Installed MAME version evidence: {}",
        text(
            report
                .analysis
                .emulator_version
                .as_deref()
                .unwrap_or("Unknown — no captured evidence")
        )
    ));
    ui.label(report.installed_version_origin);
    ui.label(match report.analysis.version_compatibility {
        MameVersionCompatibility::CatalogueVersionMatch => "Catalogue and supplied MAME release evidence match; this does not prove runnability.",
        MameVersionCompatibility::CatalogueVersionMismatch => "Catalogue and supplied MAME releases differ. Verified storage does not establish compatibility with this emulator.",
        _ => "MAME version comparison is Unknown: missing, malformed or unsupported version evidence.",
    });
    ui.horizontal_wrapped(|ui| {
        ui.strong(format!("Good {}", report.analysis.passing_sets));
        ui.strong(format!("Bad {}", report.analysis.failing_sets));
        ui.strong(format!("Unknown {}", report.analysis.unknown_sets));
    });
    ui.label(format!(
        "{} collection containers · {} catalogue sets assessed",
        report.analysis.discovered_sets,
        report.analysis.set_results.len()
    ));
    ui.label(format!(
        "CHD sets: {} requiring · {} missing · {} unverified",
        report.analysis.chd_coverage.sets_requiring_chd,
        report.analysis.chd_coverage.sets_missing_chd,
        report.analysis.chd_coverage.sets_unverified_chd
    ));
    ui.label("CHD-aware combined-digest and parent-chain verification is unavailable here. A .chd filename, header or ordinary-file hash cannot establish a healthy CHD.");
    ui.collapsing("Source identity and inspection limits", |ui| {
        ui.label(format!("Source: {}",text(&report.catalogue.reference)));
        ui.label(format!("Catalogue SHA-256: {}",report.catalogue.sha256));
        ui.label(format!("Provenance: {}",text(&report.catalogue.provenance)));
        ui.label(format!("Catalogue path: {}",text(&report.catalogue.source.path.to_string_lossy())));
        ui.label(format!("Collection: {}",text(&report.inspection.inventory.collection_root.to_string_lossy())));
        ui.label(format!("Snapshot time (Unix seconds): {}",report.inspection.inspected_at_unix_seconds));
        ui.label("ZIP and loose machine-folder reads only; symbolic links and other archive formats are refused. Cancellation is checked during hashing; catalogue parsing and backend analysis are bounded steps.");
        ui.label(format!("Limits: {} containers, {} entries, depth {}, {} GiB logical reads; catalogue 256 MiB, 100,000 machines and 500,000 declarations.",report.inspection.limits.max_containers,report.inspection.limits.max_entries,report.inspection.limits.max_depth,report.inspection.limits.max_logical_bytes / (1024 * 1024 * 1024)));
    });
}

pub(super) fn summary_text(report: &Report) -> String {
    format!(
        "MAME Collection Health — storage snapshot, not runnability\nCatalogue: {}\nSource: {}\nCatalogue SHA-256: {}\nCatalogue version: {}\nCollection: {}\nInspection complete: {}\nSnapshot Unix time: {}\nGood: {}; Bad: {}; Unknown: {}\nInstalled version evidence: {}\nVersion evidence origin: {}\nVersion comparison: {:?}\nCHD-aware verification: unavailable\n{}",
        text(&report.catalogue.display_name),
        text(&report.catalogue.reference),
        report.catalogue.sha256,
        text(
            report
                .analysis
                .catalogue_version
                .as_deref()
                .unwrap_or("Unknown")
        ),
        text(
            &report
                .inspection
                .inventory
                .collection_root
                .to_string_lossy()
        ),
        report.inspection.inventory.inspected_completely,
        report.inspection.inspected_at_unix_seconds,
        report.analysis.passing_sets,
        report.analysis.failing_sets,
        report.analysis.unknown_sets,
        text(
            report
                .analysis
                .emulator_version
                .as_deref()
                .unwrap_or("Unknown")
        ),
        report.installed_version_origin,
        report.analysis.version_compatibility,
        report
            .analysis
            .warnings
            .iter()
            .map(|w| text(w))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

fn why(ui: &mut egui::Ui, report: &Report, set: &MameSetResult) -> Option<String> {
    let mut copy_name = None;
    widgets::card(ui, |ui| {
        ui.heading(format!("Why? {}", text(&set.name)));
        ui.strong(health(set.health));
        ui.label("This classification comes from the inspected requirements below. Verified storage does not prove game runnability.");
        ui.label(format!(
            "Requirement layout evaluated: {}",
            set.evidence
                .evaluated_layout
                .map(layout_label)
                .unwrap_or("Unknown")
        ));
        let mut expected = 0usize;
        let mut no_dump = 0usize;
        let mut bad_dump = 0usize;
        let mut preservation_unknown = 0usize;
        for requirement in &set.evidence.requirements {
            match requirement.preservation {
                MamePreservationStatus::VerifiedDumpExpected => expected += 1,
                MamePreservationStatus::NoVerifiedDumpExists => no_dump += 1,
                MamePreservationStatus::NeedsRedump => bad_dump += 1,
                MamePreservationStatus::Unknown => preservation_unknown += 1,
            }
        }
        ui.label(format!(
            "Preservation: {expected} verified-dump expected · {no_dump} NO_DUMP · {bad_dump} BAD_DUMP · {preservation_unknown} unknown"
        ));
        ui.label(match set.evidence.runnability {
            MameRunnability::NotAssessed => "Runnability: not assessed; storage health does not establish whether MAME can run this machine.",
        });
        if let Some(context) = report.context.get(&set.name) {
            if let Some(description) = &context.description {
                ui.label(text(description));
            }
            ui.label(format!(
                "Clone lineage: {}",
                text(context.clone_of.as_deref().unwrap_or("No cloneof declared"))
            ));
            ui.label(format!(
                "ROM source: {}",
                text(context.rom_of.as_deref().unwrap_or("No romof declared"))
            ));
            if context.duplicate_machine_declarations {
                ui.label("Multiple machine declarations share this name. Displayed lineage is not authoritative; conflicting metadata cannot establish a complete set.");
            }
            if context.bios {
                ui.label("This is a BIOS support set.");
            }
            if context.device {
                ui.label("This is a device support set.");
            }
            for name in &context.device_references {
                ui.label(format!("Declared device dependency: {}", text(name)));
            }
        }
        if !set.evidence.inspection_complete {
            ui.label(
                "Inventory inspection is incomplete; verified completeness cannot be established.",
            );
        }
        for reason in &set.evidence.unresolved_dependencies {
            ui.label(format!("Unresolved: {}", text(reason)));
        }
        for fact in set.evidence.requirements.iter().take(200) {
            ui.separator();
            ui.strong(format!(
                "{} / {}",
                text(&fact.declaring_set),
                text(&fact.member)
            ));
            if let Some(provider) = report.context.get(&fact.declaring_set) {
                if provider.bios {
                    ui.label("Required BIOS support");
                }
                if provider.device {
                    ui.label("Required device support");
                }
            }
            ui.label(evidence(fact.content, fact.disk));
            ui.label(match fact.preservation {
                MamePreservationStatus::VerifiedDumpExpected => "A verified dump is expected by the catalogue.",
                MamePreservationStatus::NoVerifiedDumpExists => "NO_DUMP: no verified good dump is known. This is a preservation gap, not an obtainable repair.",
                MamePreservationStatus::NeedsRedump => "BAD_DUMP: the recorded dump is imperfect and needs redumping; matching it is not a clean preservation result.",
                MamePreservationStatus::Unknown => "Preservation status is unsupported or unresolved; no acquisition opportunity is established.",
            });
        }
        if set.evidence.requirements.len() > 200 {
            ui.label(
                "First 200 requirements shown. The copied set receipt contains every requirement.",
            );
        }
        ui.collapsing("Recorded and inspected checksums", |ui| {
            let mut providers = std::collections::BTreeSet::from([set.name.as_str()]);
            providers.extend(set.evidence.requirements.iter().map(|r| r.declaring_set.as_str()));
            let mut rows = 0usize;
            for provider in providers {
                if let Some(context) = report.context.get(provider) {
                    for rom in &context.rom_declarations {
                        if rows >= 200 { break; }
                        rows += 1;
                        ui.label(format!("Recorded ROM {} / {}: size {}; CRC32 {}; MD5 {}; SHA-1 {}; status {}", text(provider), text(&rom.name), rom.size_bytes.map(|n| n.to_string()).unwrap_or_else(|| "not recorded".into()), text(rom.crc32.as_deref().unwrap_or("not recorded")), text(rom.md5.as_deref().unwrap_or("not recorded")), text(rom.sha1.as_deref().unwrap_or("not recorded")), text(rom.status.as_deref().unwrap_or("not recorded"))));
                    }
                    for disk in &context.disk_declarations {
                        if rows >= 200 { break; }
                        rows += 1;
                        ui.label(format!("Recorded CHD {} / {}: combined SHA-1 {} (not verified by this inspection)", text(provider), text(disk.name.as_deref().unwrap_or("unnamed")), text(disk.sha1.as_deref().unwrap_or("not recorded"))));
                    }
                }
                for container in report.inspection.inventory.sets.iter().filter(|s| s.name == provider) {
                    for member in &container.members {
                        if rows >= 200 { break; }
                        rows += 1;
                        ui.label(format!("Observed {} / {}: size {}; CRC32 {}; MD5 {}; SHA-1 {}", text(provider), text(&member.name), member.size_bytes.map(|n| n.to_string()).unwrap_or_else(|| "unverified".into()), text(member.crc32.as_deref().unwrap_or("unverified")), text(member.md5.as_deref().unwrap_or("unverified")), text(member.sha1.as_deref().unwrap_or("unverified"))));
                    }
                }
                if rows >= 200 { break; }
            }
            ui.label("These are recorded declarations and observed container contents. The requirement verdict above uses the backend's dependency and merge resolution; a matching name alone is not a binding. First 200 checksum rows shown.");
        });
        if ui.button("Copy this set's evidence").clicked() {
            copy_name = Some(set.name.clone());
        }
    });
    copy_name
}

fn health(health: MameSetHealth) -> &'static str {
    match health {
        MameSetHealth::Good => "Good — verified storage snapshot",
        MameSetHealth::Bad => "Bad — missing or contradictory content",
        MameSetHealth::Unknown => "Unknown — evidence does not establish completeness",
    }
}

fn layout_label(layout: MameCollectionStyle) -> &'static str {
    match layout {
        MameCollectionStyle::Merged => "merged",
        MameCollectionStyle::Split => "split",
        MameCollectionStyle::NonMerged => "non-merged",
        MameCollectionStyle::ExplodedSelfContained => "exploded self-contained",
        MameCollectionStyle::Mixed => "mixed",
        MameCollectionStyle::Unknown => "unknown",
    }
}
fn evidence(e: MameMemberEvidence, disk: bool) -> &'static str {
    match e {
        MameMemberEvidence::Verified => {
            "Verified: inspected bytes match the required strong hashes and declared size."
        }
        MameMemberEvidence::Missing if disk => {
            "CHD missing: no required disk was observed in the inspected locations."
        }
        MameMemberEvidence::Missing => {
            "Missing ROM: no required member was observed in the inspected locations."
        }
        MameMemberEvidence::Contradictory => {
            "Contradictory content: a required checksum or size disagrees with inspected bytes."
        }
        MameMemberEvidence::WeakHash => {
            "Unverified: CRC32 alone is insufficient to establish content identity."
        }
        MameMemberEvidence::PartialHash => {
            "Unverified: some required checksum or size evidence is unavailable."
        }
        MameMemberEvidence::NameOrSizeOnly => {
            "Unverified: a matching filename or size is not verified content."
        }
        MameMemberEvidence::Uninspectable if disk => {
            "CHD unverified: presence does not establish its combined digest or parent chain."
        }
        MameMemberEvidence::Uninspectable => {
            "Unverified: incomplete, conflicting, unsupported or ambiguous evidence; no authority to choose a matching copy."
        }
    }
}
