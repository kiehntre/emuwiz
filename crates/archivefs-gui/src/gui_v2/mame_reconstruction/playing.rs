//! Read-only MAME Playing Library handoff. The existing inspector and resolver
//! own storage/dependency semantics; the native planner/executor owns launching.
//! A library or reconstruction receipt is never an execution verdict.
use super::{
    MameEnvironment, MameInputs,
    catalogue::{self, CatalogueBinding},
};
use crate::{
    emulator_setup_overrides::EmulatorPathOverrides, launch_readiness_page::StandaloneLaunchState,
};
use archivefs_core::{
    arcade_mame_compatibility::{MameCollectionLayout, MameExpectationIndex},
    dat::{
        dependency::{DependencyState, DependencyTarget, SetDependencyReport},
        set::{SetIdentity, SetResolution, SetState},
    },
    launch::{
        CanonicalIdentityStatus, MameLaunchRequest, ResolvedIdentity, build_mame_command_plan,
        preflight_mame_launch,
    },
    mame_collection_analyser::{
        MameCollectionInventory, MameCollectionStyle, MameSetResult, analyse_collection,
        inspection::{InspectionLimits, SourceStamp, inspect_collection},
    },
    mame_playing_library::{
        MamePlayingLibraryPreferences, MamePlayingLibraryRequest, build_mame_playing_library_plan,
    },
};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::AtomicBool,
        mpsc::{self, Receiver},
    },
};

#[derive(Clone, Debug)]
pub(super) struct Machine {
    pub name: String,
    title: String,
    normal: bool,
    pub result: MameSetResult,
    anchor: Option<PathBuf>,
    samples: bool,
    recommended: bool,
}
#[derive(Clone, Debug)]
pub(super) struct Report {
    inputs: MameInputs,
    binding: CatalogueBinding,
    pub(super) inventory: MameCollectionInventory,
    sources: Vec<SourceStamp>,
    pub machines: Vec<Machine>,
}

pub(super) fn inspect(
    inputs: &MameInputs,
    environment: &MameEnvironment,
) -> Result<Report, String> {
    let selected = catalogue::load_selected(inputs, environment)?;
    let cancel = AtomicBool::new(false);
    let inspection = inspect_collection(
        &inputs.root,
        InspectionLimits::default(),
        &cancel,
        &mut |_| {},
    )?;
    let analysis = analyse_collection(&selected.dat.parsed, &inspection.inventory, None, None);
    let curated = build_mame_playing_library_plan(&MamePlayingLibraryRequest {
        catalogue: &selected.dat.parsed,
        inventory: &inspection.inventory,
        analysis: &analysis,
        preferences: MamePlayingLibraryPreferences::default(),
    })?;
    let index = MameExpectationIndex::new(&selected.dat.parsed);
    let machines = analysis
        .set_results
        .into_iter()
        .filter_map(|result| {
            let declarations: Vec<_> = selected
                .dat
                .parsed
                .games
                .iter()
                .filter(|g| g.name == result.name)
                .collect();
            let [game] = declarations.as_slice() else {
                return None;
            };
            let yes = |value: &Option<String>| {
                value
                    .as_deref()
                    .is_some_and(|s| s.trim().eq_ignore_ascii_case("yes"))
            };
            // Physical packing comes only from resolver-approved layout locations.
            // cloneof is neither a ROM source nor a guessed archive pathname here.
            let layout = match result.evidence.evaluated_layout {
                Some(MameCollectionStyle::Split) => MameCollectionLayout::Split,
                Some(MameCollectionStyle::Merged) => MameCollectionLayout::Merged,
                Some(MameCollectionStyle::NonMerged) => MameCollectionLayout::NonMerged,
                _ => MameCollectionLayout::RuntimeSearch,
            };
            let expectation = index.expectation_for_layout(&game.name, layout);
            let anchor = expectation.as_ref().and_then(|e| {
                let mut requirements = e
                    .dependencies
                    .resolution
                    .roms
                    .iter()
                    .filter(|r| r.declaring_set == game.name)
                    .collect::<Vec<_>>();
                // Prefer the machine's own program content over a BIOS/device
                // support container; all alternatives still come from the resolver.
                requirements.sort_by_key(|r| !matches!(&r.owner, Some(DependencyTarget::SetMember { set_name, .. }) if set_name == &game.name));
                requirements.into_iter().find_map(|requirement| {
                    let expected = e.roms.iter().find(|rom| rom.set_name == requirement.declaring_set && rom.name == requirement.member_name)?;
                    let sha1 = expected.checksums.iter().find(|h| h.algorithm == archivefs_core::dat::model::ChecksumAlgorithm::Sha1)?;
                    requirement.locations.iter().find_map(|location| {
                        // A search candidate is not evidence. The chosen anchor
                        // must actually contain this exact SHA-1/size-bound ROM.
                        // Permission to launch still comes from the complete
                        // collection verdict, never this single member alone.
                        inspection.inventory.sets.iter().find(|set| set.name == location.set_name
                            && set.location.parent() == Some(inputs.root.as_path())
                            && set.members.iter().any(|member| member.name == location.member_name
                                && member.sha1.as_ref().is_some_and(|observed| observed.eq_ignore_ascii_case(&sha1.value))
                                && expected.size_bytes.is_none_or(|size| member.size_bytes == Some(size))))
                            .map(|set| set.location.clone())
                    })
                })
            });
            Some(Machine {
                name: game.name.clone(),
                title: game
                    .description
                    .clone()
                    .unwrap_or_else(|| game.name.clone()),
                // Retain the existing MAME join's absent/yes convention;
                // an explicit no or malformed runnable value refuses.
                normal: game
                    .runnable
                    .as_deref()
                    .is_none_or(|s| s.trim().eq_ignore_ascii_case("yes"))
                    && !yes(&game.is_bios)
                    && !yes(&game.is_device)
                    && !game
                        .original_metadata
                        .fields
                        .get("ismechanical")
                        .is_some_and(|s| s.trim().eq_ignore_ascii_case("yes")),
                samples: game.sample_of.is_some() || !game.samples.is_empty(),
                recommended: curated.selected_sets.iter().any(|s| s.name == game.name),
                result,
                anchor,
            })
        })
        .collect();
    if !selected.binding.source.still_matches() || !inspection.sources_unchanged(&cancel) {
        return Err(
            "Collection or catalogue changed during inspection; refresh Playing Library.".into(),
        );
    }
    Ok(Report {
        inputs: inputs.clone(),
        binding: selected.binding,
        inventory: inspection.inventory,
        sources: inspection.sources,
        machines,
    })
}

pub(super) fn request(
    report: &Report,
    machine: &str,
    executable: PathBuf,
) -> Result<MameLaunchRequest, String> {
    let item = report
        .machines
        .iter()
        .find(|m| m.name == machine)
        .ok_or("Machine is unavailable; refresh Playing Library.")?;
    if !item.normal {
        return Err("The catalogue does not declare an ordinary runnable machine; BIOS/device/mechanical/unknown declarations cannot authorize this launch.".into());
    }
    let content = item.anchor.clone().ok_or("No verified-layout physical container is available for this machine. Review collection health or reconstruction.")?;
    let evidence = &item.result.evidence;
    let verified = evidence.verified_complete();
    let required = |disk: bool| {
        evidence
            .requirements
            .iter()
            .filter(move |r| r.disk == disk)
            .map(|r| format!("{}/{}", r.declaring_set, r.member))
            .collect::<Vec<_>>()
    };
    let resolution = SetResolution {
        identity: SetIdentity {
            source_id: format!(
                "mame-arcade:{}:{}",
                report.binding.version, report.binding.sha256
            ),
            game_name: item.name.clone(),
        },
        archive_path: content.clone(),
        state: if verified {
            SetState::Complete
        } else {
            SetState::Incomplete
        },
        members_required: required(false),
        members_verified: if verified {
            required(false)
        } else {
            Vec::new()
        },
        members_bad: Vec::new(),
        members_optional: Vec::new(),
        members_borrowed: Vec::new(),
        disks_required: required(true),
        disks_verified: if verified { required(true) } else { Vec::new() },
        disks_parent_required: Vec::new(),
        dependencies: SetDependencyReport {
            state: if verified {
                DependencyState::Satisfied
            } else {
                DependencyState::EvidenceUnavailable
            },
            requirements: Vec::new(),
        },
    };
    let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "Arcade".into(),
        game_key: item.name.clone(),
    });
    let plan = build_mame_command_plan(
        &identity,
        std::slice::from_ref(&resolution),
        Some(&executable),
        Some(&report.inputs.root),
    );
    if !plan.blockers.is_empty() {
        return Err(plan
            .blockers
            .iter()
            .map(|b| b.detail.clone())
            .collect::<Vec<_>>()
            .join("; "));
    }
    // Carry the existing executor's regular-file identity check forward to
    // its last preflight. Directory members still require the fresh inspector;
    // neither branch claims exclusion of later external writers.
    let stamp = report
        .sources
        .iter()
        .find(|s| s.path == content)
        .ok_or("Physical container identity unavailable; refresh Playing Library.")?;
    if !stamp.still_matches() {
        return Err("Physical container changed; refresh Playing Library.".into());
    }
    let expected_content_identity = if stamp.directory {
        None
    } else {
        let (_, identity) = archivefs_core::launch::process_spawn::capture_file_identity(&content)
            .map_err(|e| e.to_string())?;
        if !stamp.still_matches() {
            return Err("Physical container changed while preparing launch.".into());
        }
        Some(identity)
    };
    let request = MameLaunchRequest {
        identity,
        set_resolutions: vec![resolution],
        expected_executable: executable,
        selected_content: content,
        expected_content_identity,
        rom_search_path: report.inputs.root.clone(),
    };
    preflight_mame_launch(&request).map_err(|e| {
        e.blockers
            .iter()
            .map(|b| b.detail.clone())
            .collect::<Vec<_>>()
            .join("; ")
    })?;
    Ok(request)
}

/// Rehash the collection and resolve the exact installed catalogue again in
/// the launch worker. Metadata stamps alone never authorize changed content.
pub(super) fn revalidate(
    report: &Report,
    environment: &MameEnvironment,
    machine: &str,
    executable: PathBuf,
) -> Result<MameLaunchRequest, String> {
    let fresh = inspect(&report.inputs, environment)?;
    if fresh.binding != report.binding
        || fresh.inventory != report.inventory
        || fresh.sources != report.sources
    {
        return Err("Catalogue or collection evidence changed; refresh Playing Library and review the launch again.".into());
    }
    request(&fresh, machine, executable)
}

#[derive(Default)]
pub(super) struct Panel {
    pub(super) report: Option<Arc<Report>>,
    job: Option<Receiver<Result<Report, String>>>,
    selected: String,
    query: String,
    confirm: bool,
    displayed_executable: Option<PathBuf>,
    message: Option<String>,
    launch: StandaloneLaunchState,
    generation: u64,
    job_generation: u64,
    pending_refresh: Option<(MameInputs, MameEnvironment)>,
}
impl Panel {
    fn bind_executable(&mut self, executable: Option<PathBuf>) {
        if self.displayed_executable != executable {
            self.confirm = false;
            self.displayed_executable = executable;
        }
    }
    pub(super) fn invalidate(&mut self) {
        self.generation += 1;
        self.report = None;
        self.confirm = false;
        self.pending_refresh = None;
    }
    pub(super) fn refresh(&mut self, inputs: MameInputs, environment: MameEnvironment) {
        self.invalidate();
        if self.job.is_some() {
            self.pending_refresh = Some((inputs, environment));
            return;
        }
        self.job_generation = self.generation;
        let (tx, rx) = mpsc::channel();
        self.job = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(inspect(&inputs, &environment));
        });
    }
    pub(super) fn launch_active(&self) -> bool {
        self.launch.is_active()
    }
    pub(super) fn busy(&self) -> bool {
        self.job.is_some() || self.launch.is_active()
    }
    pub(super) fn poll(&mut self) {
        self.launch.poll();
        if let Some(receiver) = &self.job {
            match receiver.try_recv() {
                Ok(result) => {
                    self.job = None;
                    if self.job_generation == self.generation {
                        match result {
                            Ok(report) => {
                                self.message = None;
                                self.report = Some(Arc::new(report));
                            }
                            Err(e) => {
                                self.message = Some(e);
                            }
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.job = None;
                    self.message =
                        Some("Playing Library inspection stopped; no launch authorized.".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.job.is_none()
            && let Some((inputs, environment)) = self.pending_refresh.take()
        {
            self.refresh(inputs, environment);
        }
    }
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        inputs: Option<MameInputs>,
        environment: &MameEnvironment,
        mutation_busy: bool,
    ) -> Option<String> {
        self.poll();
        if let (Some(report), Some(inputs)) = (&self.report, &inputs)
            && (report.inputs.root != inputs.root
                || report.inputs.dat != inputs.dat
                || report.inputs.reference != inputs.reference
                || report.inputs.trusted != inputs.trusted)
        {
            self.invalidate();
        }
        if inputs.is_none() && self.report.is_some() {
            self.invalidate();
        }
        ui.heading("MAME Playing Library");
        ui.label("Choose the collection and trusted catalogue above, then refresh. Select a machine, not a raw ROM member. Verified ROM completeness does not prove that a machine boots or is independently playable.");
        if ui
            .add_enabled(
                inputs.as_ref().is_some_and(|i| i.trusted) && !self.busy() && !mutation_busy,
                egui::Button::new("Refresh MAME Playing Library"),
            )
            .clicked()
        {
            self.refresh(inputs.clone().unwrap(), environment.clone());
        }
        if self.job.is_some() {
            ui.spinner();
        }
        if self.job.is_some() || self.launch.is_active() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        if let Some(message) = &self.message {
            ui.label(message);
        }
        let mut reconstruct = None;
        if let Some(report) = self.report.clone() {
            ui.label(format!(
                "Catalogue {} · SHA-256 {}",
                report.binding.version, report.binding.sha256
            ));
            ui.horizontal(|ui| {
                ui.label("Find machine");
                ui.text_edit_singleline(&mut self.query);
            });
            ui.small(
                "Showing up to 200 matching machines; narrow the search by title or shortname.",
            );
            let query = self.query.trim().to_lowercase();
            let previous = self.selected.clone();
            egui::ComboBox::from_id_salt("mame_playing_machine")
                .selected_text(if self.selected.is_empty() {
                    "Select a machine"
                } else {
                    &self.selected
                })
                .show_ui(ui, |ui| {
                    for machine in report
                        .machines
                        .iter()
                        .filter(|m| {
                            query.is_empty()
                                || m.name.to_lowercase().contains(&query)
                                || m.title.to_lowercase().contains(&query)
                        })
                        .take(200)
                    {
                        ui.selectable_value(
                            &mut self.selected,
                            machine.name.clone(),
                            format!(
                                "{} — {}{}",
                                machine.name,
                                machine.title,
                                if machine.recommended {
                                    " (curated choice)"
                                } else {
                                    ""
                                }
                            ),
                        );
                    }
                });
            if previous != self.selected {
                self.confirm = false;
                self.message = None;
            }
            if let Some(machine) = report.machines.iter().find(|m| m.name == self.selected) {
                ui.label(if machine.result.evidence.verified_complete() {
                    "Last inspection: required ROM storage verified · machine runnability not assessed"
                } else {
                    "ROM/dependency evidence incomplete or unverified · launch refused"
                });
                for requirement in &machine.result.evidence.requirements {
                    ui.label(format!(
                        "{}/{}: {:?} · {:?}",
                        requirement.declaring_set,
                        requirement.member,
                        requirement.content,
                        requirement.preservation
                    ));
                }
                for reason in &machine.result.evidence.unresolved_dependencies {
                    ui.label(reason);
                }
                if machine.samples {
                    ui.label("Sample sets are separate and not verified by this inspection; sound completeness is not established.");
                }
                let executable = EmulatorPathOverrides::load().mame_executable;
                self.bind_executable(executable.clone());
                let review = executable.clone().ok_or_else(|| "Configure the native MAME executable in Emulator Setup first; launch overrides remain unchanged.".into()).and_then(|e| request(&report, &self.selected, e));
                match &review {
                    Ok(r) => {
                        ui.label(format!(
                            "Native MAME · {} · machine {} · ROM search {}",
                            r.expected_executable.display(),
                            self.selected,
                            r.rom_search_path.display()
                        ));
                    }
                    Err(e) => {
                        ui.label(e);
                    }
                }
                if let Ok(request) = &review {
                    ui.label(format!(
                        "Physical container: {}",
                        request.selected_content.display()
                    ));
                }
                ui.label("Installed MAME version compatibility and actual boot are not established by ROM storage checks. Use the MAME build matching this catalogue.");
                ui.checkbox(
                    &mut self.confirm,
                    "I reviewed this machine, catalogue and configured MAME executable",
                );
                if ui
                    .add_enabled(
                        review.is_ok()
                            && self.confirm
                            && !mutation_busy
                            && !self.launch.is_active(),
                        egui::Button::new("Launch selected MAME machine"),
                    )
                    .clicked()
                {
                    let environment = environment.clone();
                    let machine = self.selected.clone();
                    let content = review.unwrap().selected_content;
                    self.launch.start_mame_revalidated(content, move || {
                        revalidate(&report, &environment, &machine, executable.unwrap())
                    });
                    self.confirm = false;
                }
                if ui
                    .button("Review reconstruction for this machine above")
                    .clicked()
                {
                    reconstruct = Some(self.selected.clone());
                }
            }
        }
        self.launch.show_mame_feedback(ui);
        reconstruct
    }
}

#[cfg(test)]
mod confirmation_tests {
    use super::*;

    #[test]
    fn changing_the_displayed_executable_requires_fresh_confirmation() {
        let mut panel = Panel::default();
        // First display and explicit review of executable A.
        panel.bind_executable(Some(PathBuf::from("/synthetic/mame-A")));
        panel.confirm = true;
        // Another frame with the same binding preserves that review.
        panel.bind_executable(Some(PathBuf::from("/synthetic/mame-A")));
        assert!(panel.confirm);
        // Returning from Setup with B must not carry A's confirmation.
        panel.bind_executable(Some(PathBuf::from("/synthetic/mame-B")));
        assert!(!panel.confirm);
        panel.confirm = true;
        panel.bind_executable(None);
        assert!(!panel.confirm);
        // Even checking the box while unavailable cannot approve a new binding.
        panel.confirm = true;
        panel.bind_executable(Some(PathBuf::from("/synthetic/mame-B")));
        assert!(!panel.confirm);
    }
}
