//! Page-owned projection and explicit provider checks. No database handle is
//! passed to the worker; the only writes belong to existing check/cache APIs.
use super::*;
use archivefs_core::dat::update_health::{
    Assessment, Check, CheckHistory, CheckResult, PlanDisposition, assess_health,
    check_from_managed_outcome, plan_update_all,
};
use archivefs_core::dat::updates::{load_managed_dat_state, resolve_current_managed_dat_source};

#[derive(Default)]
pub(super) struct Coordinator {
    histories: BTreeMap<String, CheckHistory>,
    cooldowns: BTreeMap<String, u64>,
    pub(super) job: Option<Receiver<BatchResult>>,
}

pub(super) struct BatchResult {
    checks: Vec<(ManagedDatSourceId, Check)>,
    cooldowns: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HealthRow {
    pub(crate) name: String,
    pub(crate) source_id: Option<ManagedDatSourceId>,
    pub(crate) installed_version: Option<String>,
    pub(crate) sha256: Option<String>,
    pub(crate) last_checked_display: Option<String>,
    pub(crate) assessment: Assessment,
    pub(crate) plan: PlanDisposition,
    pub(crate) explanation: String,
}

fn service(provider: ManagedDatProvider) -> &'static str {
    match provider {
        ManagedDatProvider::MameSoftwareList => "github",
        ManagedDatProvider::RedumpBios | ManagedDatProvider::RedumpGames => "redump",
        ManagedDatProvider::Fbneo => "local",
    }
}

fn supports_check(row: &ManagedDatSourceRowView) -> bool {
    row.configured
        && row.installed
        && row.update_policy != ManagedDatUpdatePolicy::Disabled
        && match row.provider {
            ManagedDatProvider::MameSoftwareList | ManagedDatProvider::RedumpBios => true,
            ManagedDatProvider::RedumpGames => {
                redump_game_from_source_key(&row.source_id.source_key).is_some_and(|system| {
                    system.acquisition_mode() == RedumpAcquisitionMode::RemoteManaged
                })
            }
            ManagedDatProvider::Fbneo => false,
        }
}

fn cached_no_intro_metadata(
    status: &archivefs_core::identity_source::no_intro::ManagedNoIntroStatusReport,
    root: &Path,
    path: &Path,
) -> Option<(Option<String>, Option<String>, bool)> {
    for snapshot in &status.snapshots {
        for (index, member) in snapshot.members.iter().enumerate() {
            if root
                .join("snapshots")
                .join(&snapshot.snapshot_sha256)
                .join("dats")
                .join(format!("{index}.dat"))
                == path
            {
                return Some((
                    member.upstream_version.clone(),
                    Some(member.dat_member_sha256.clone()),
                    snapshot.invalid || snapshot.unresolved,
                ));
            }
        }
    }
    None
}

impl DatSourcesPageState {
    /// Reuse cached pack provenance only for the exact immutable member path.
    /// A lifecycle's "current" means active, and never supplies freshness here.
    fn cached_pack_metadata(
        &self,
        entry: &DatSourceEntry,
    ) -> (Option<String>, Option<String>, bool) {
        let path = &entry.path;
        if let Some(status) = &self.no_intro_status
            && let Ok(root) = archivefs_core::app_dirs::data_path("no_intro_pack")
            && let Some(metadata) = cached_no_intro_metadata(status, &root, path)
        {
            return metadata;
        }
        if let Some(sha256) = self.tosec_managed_view.active_sha256.as_deref()
            && archivefs_core::dat::update_health::valid_sha256(sha256)
            && let Some((_, relative)) = entry.ownership.imported_tosec_release_pack()
            && relative
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
            && self
                .managed_root
                .join("tosec-snapshots")
                .join("materialized")
                .join(sha256)
                .join(relative)
                == *path
        {
            return (self.tosec_managed_view.active_release.clone(), None, false);
        }
        (None, None, false)
    }

    pub(super) fn managed_health_row(&self, id: &ManagedDatSourceId) -> Option<HealthRow> {
        self.view()
            .update_health
            .into_iter()
            .find(|row| row.source_id.as_ref() == Some(id))
    }

    pub(super) fn health_rows(
        &self,
        mame: &[ManagedDatSourceRowView],
        bios: &[ManagedDatSourceRowView],
        games: &[ManagedDatSourceRowView],
        fbneo: &[ManagedDatSourceRowView],
    ) -> Vec<HealthRow> {
        let now = now_unix_seconds();
        let managed = mame
            .iter()
            .chain(bios)
            .chain(games)
            .chain(fbneo)
            .collect::<Vec<_>>();
        let mut claims: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for entry in self.draft.sorted_all() {
            if let Some(report) = self.validations.get(&entry.id) {
                for file in &report.files {
                    if let DatFileOutcome::Parsed {
                        ecosystem,
                        name: Some(name),
                        ..
                    } = &file.outcome
                    {
                        if !name.trim().is_empty() {
                            claims
                                .entry(format!("{ecosystem:?}:{name}"))
                                .or_default()
                                .insert(entry.id.clone());
                        }
                    }
                }
            }
        }
        let conflicting_ids = claims
            .values()
            .filter(|ids| ids.len() > 1)
            .flatten()
            .cloned()
            .collect::<BTreeSet<_>>();
        let conflicting_paths = self
            .draft
            .sorted_all()
            .into_iter()
            .filter(|entry| conflicting_ids.contains(&entry.id))
            .map(|entry| &entry.path)
            .collect::<BTreeSet<_>>();
        let mut rows = Vec::new();
        for row in managed.iter().filter(|row| {
            row.configured
                && (row.installed || matches!(row.status, ManagedDatStatusView::Failed { .. }))
        }) {
            let history = self
                .update_health
                .histories
                .get(&managed_source_key(&row.source_id))
                .cloned()
                .unwrap_or_default();
            let assessment = assess_health(
                row.technical.sha256.as_deref().filter(|_| row.installed),
                &history,
                now,
            );
            let conflict = !row.installed
                || row
                    .technical
                    .current_path
                    .as_deref()
                    .is_some_and(|path| conflicting_paths.contains(&PathBuf::from(path)));
            rows.push(HealthRow {
                name: row.source_label.clone(), source_id: Some(row.source_id.clone()),
                installed_version: row.current_revision.clone(), sha256: row.technical.sha256.clone(),
                last_checked_display: assessment.last_checked.map(format_unix_timestamp).or_else(|| row.last_checked.clone()),
                plan: plan_update_all(&assessment, supports_check(row), conflict), assessment,
                explanation: if conflict { "Managed metadata or snapshot could not be resolved; review this source." }
                    else if !supports_check(row) { "No supported automatic provider check. Use this source's explicit local import/browser workflow." }
                    else { "Existing managed provider; update-all is a read-only plan." }.into(),
            });
        }
        // Registry projections of the exact managed snapshot are one catalogue,
        // not a second installation. Never infer ownership from a display name.
        for entry in self.draft.sorted_all() {
            if entry.ownership.is_emuwiz_managed()
                && managed.iter().any(|row| {
                    row.technical
                        .current_path
                        .as_deref()
                        .is_some_and(|path| Path::new(path) == entry.path)
                })
            {
                continue;
            }
            let report = self.validations.get(&entry.id);
            let (pack_version, pack_sha256, pack_conflict) = self.cached_pack_metadata(entry);
            let mut versions = report
                .into_iter()
                .flat_map(|report| &report.files)
                .filter_map(|file| {
                    if let DatFileOutcome::Parsed { version, .. } = &file.outcome {
                        version.clone()
                    } else {
                        None
                    }
                })
                .collect::<BTreeSet<_>>();
            if versions.is_empty() {
                versions.extend(
                    entry
                        .health
                        .arcade_catalogue_revisions
                        .iter()
                        .filter_map(|revision| revision.version.clone()),
                );
            }
            if versions.is_empty() {
                versions.extend(pack_version);
            }
            let conflict = pack_conflict
                || std::fs::metadata(&entry.path).is_err()
                || conflicting_ids.contains(&entry.id)
                || report.is_some_and(|report| !report.duplicate_identities.is_empty())
                || matches!(
                    entry.health.state(),
                    DatHealthState::Invalid | DatHealthState::Unreadable
                )
                || entry.ownership.is_emuwiz_managed();
            let assessment = assess_health(None, &CheckHistory::default(), now);
            rows.push(HealthRow {
                name: entry.display_name.clone(), source_id: None,
                installed_version: if versions.is_empty() { None } else { Some(versions.into_iter().collect::<Vec<_>>().join(", ")) },
                sha256: pack_sha256, last_checked_display: None, plan: plan_update_all(&assessment, false, conflict), assessment,
                explanation: if conflict { "Conflicting catalogue identity, invalid validation, or unresolved managed ownership; review before updating." }
                    else { "Local/imported catalogue. Remote comparison is unsupported; validation evidence remains separate." }.into(),
            });
        }
        rows
    }

    pub(super) fn start_health_checks(&mut self, only: Option<ManagedDatSourceId>) {
        if self.is_busy() || self.managed_load_error.is_some() {
            return;
        }
        let view = self.view();
        let rows = view
            .managed_rows
            .iter()
            .chain(&view.redump_bios_rows)
            .chain(&view.redump_game_rows)
            .chain(&view.fbneo_rows);
        let tasks = match self.managed_sources.descriptors() {
            Ok(descriptors) => descriptors
                .into_iter()
                .filter_map(|descriptor| {
                    if only.as_ref().is_some_and(|id| id != descriptor.source_id()) {
                        return None;
                    }
                    let row = rows
                        .clone()
                        .find(|row| row.source_id == *descriptor.source_id())?;
                    if !supports_check(row)
                        || view.update_health.iter().any(|health| {
                            health.source_id.as_ref() == Some(descriptor.source_id())
                                && health.plan == PlanDisposition::Conflict
                        })
                    {
                        return None;
                    }
                    Some((descriptor, row.technical.sha256.clone()?))
                })
                .collect::<Vec<_>>(),
            Err(error) => {
                self.managed_action_error = Some(error.to_string());
                return;
            }
        };
        if tasks.is_empty() {
            self.managed_action_error = Some("No installed catalogues support a provider check. Review the read-only update-all plan.".into());
            return;
        }
        self.managed_action_error = None;
        let root = self.managed_root.clone();
        let cooldowns = self.update_health.cooldowns.clone();
        let (sender, receiver) = sync_channel(1);
        std::thread::spawn(move || {
            let result = run_checks(
                tasks,
                cooldowns,
                || now_unix_seconds(),
                |descriptor, hash| {
                    checked_provider(
                        &root,
                        descriptor,
                        hash,
                        &HttpsManagedDatTransport::default(),
                    )
                },
                || std::thread::sleep(std::time::Duration::from_secs(1)),
            );
            let _ = sender.send(result);
        });
        self.update_health.job = Some(receiver);
    }

    pub(super) fn poll_health_checks(&mut self) -> bool {
        let Some(receiver) = &self.update_health.job else {
            return false;
        };
        match receiver.try_recv() {
            Ok(batch) => {
                for (id, check) in batch.checks {
                    let key = managed_source_key(&id);
                    // Reuse existing per-source status presentation, while health
                    // keeps its own hash-bound evidence and timestamps.
                    let status = match &check.result {
                        CheckResult::Current => ManagedDatStatusView::UpToDate,
                        CheckResult::UpdateAvailable { available_version } => {
                            ManagedDatStatusView::UpdateAvailable {
                                upstream_revision: available_version.clone(),
                            }
                        }
                        CheckResult::Failed { detail } => ManagedDatStatusView::Failed {
                            detail: detail.clone(),
                        },
                        CheckResult::Unknown => ManagedDatStatusView::Idle,
                    };
                    self.managed_statuses.insert(key.clone(), status);
                    self.update_health
                        .histories
                        .entry(key)
                        .or_default()
                        .record(check);
                }
                self.update_health.cooldowns = batch.cooldowns;
                self.update_health.job = None;
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.managed_action_error =
                    Some("DAT check worker stopped before returning results. Check again.".into());
                self.update_health.job = None;
                true
            }
        }
    }
}

/// Resolve and verify the installed object off the GUI thread before and
/// after a check. A metadata-only check must never validate corrupt bytes.
fn installed_hash(
    root: &Path,
    descriptor: &ManagedDatSourceDescriptor,
) -> archivefs_core::Result<String> {
    let state = load_managed_dat_state(root, descriptor)?;
    let source = resolve_current_managed_dat_source(root, &state)?;
    let mut file =
        File::open(source.path()).map_err(|error| archivefs_core::ArchiveFsError::Io {
            path: Some(source.path().to_path_buf()),
            source: error,
        })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| archivefs_core::ArchiveFsError::Io {
                path: Some(source.path().to_path_buf()),
                source: error,
            })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let hash: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if hash != state.sha256 {
        return Err(archivefs_core::ArchiveFsError::Config(
            "Installed snapshot SHA-256 does not match its provenance".into(),
        ));
    }
    Ok(hash)
}

fn checked_provider(
    root: &Path,
    descriptor: &ManagedDatSourceDescriptor,
    expected_hash: &str,
    transport: &dyn archivefs_core::dat::updates::ManagedDatTransport,
) -> archivefs_core::Result<ManagedDatUpdateOutcome> {
    if installed_hash(root, descriptor)? != expected_hash {
        return Err(archivefs_core::ArchiveFsError::Config(
            "Installed catalogue changed before the check".into(),
        ));
    }
    let result = check_managed_dat_update(
        descriptor,
        &ManagedDatUpdateOptions::new(root.to_path_buf(), now_unix_seconds()),
        transport,
    )?;
    if installed_hash(root, descriptor)? != expected_hash {
        return Err(archivefs_core::ArchiveFsError::Config(
            "Installed catalogue changed during the check".into(),
        ));
    }
    Ok(result)
}

/// Bounded concurrency of one. Rate limits apply across datasets sharing a
/// service and across explicit runs; never busy-wait on a provider's cooldown.
fn run_checks(
    tasks: Vec<(ManagedDatSourceDescriptor, String)>,
    mut cooldowns: BTreeMap<String, u64>,
    now: impl Fn() -> u64,
    mut check: impl FnMut(
        &ManagedDatSourceDescriptor,
        &str,
    ) -> archivefs_core::Result<ManagedDatUpdateOutcome>,
    mut pace: impl FnMut(),
) -> BatchResult {
    let mut checks = Vec::new();
    let mut contacted = BTreeSet::new();
    for (descriptor, hash) in tasks {
        let service = service(descriptor.source_id().provider).to_string();
        let time = now();
        let result = if cooldowns.get(&service).is_some_and(|until| *until > time) {
            Ok(ManagedDatUpdateOutcome::RateLimited {
                retry_after_seconds: Some(cooldowns[&service] - time),
            })
        } else {
            if !contacted.insert(service.clone()) {
                pace();
            }
            check(&descriptor, &hash)
        };
        let time = now();
        if let Ok(ManagedDatUpdateOutcome::RateLimited {
            retry_after_seconds,
        }) = &result
        {
            cooldowns.insert(
                service,
                time.saturating_add(retry_after_seconds.unwrap_or(60).max(1)),
            );
        }
        let check = match result {
            Ok(outcome) => check_from_managed_outcome(&outcome, hash, time),
            Err(error) => Check {
                installed_sha256: hash,
                checked_at: time,
                available_version: None,
                result: CheckResult::Failed {
                    detail: error.to_string(),
                },
            },
        };
        checks.push((descriptor.source_id().clone(), check));
    }
    BatchResult { checks, cooldowns }
}

pub(super) fn show(ui: &mut egui::Ui, view: &DatSourcesPageView) -> Option<DatSourcesPageAction> {
    let mut action = None;
    widgets::full_width_card(ui, |ui| {
        ui.strong("Installed catalogue updates & health");
        ui.label("Provider checks run only when requested. Check All never replaces DATs or rechecks games.");
        if ui
            .add_enabled(
                !view.background_busy && view.managed_load_error.is_none(),
                egui::Button::new("Check All"),
            )
            .clicked()
        {
            action = Some(DatSourcesPageAction::CheckAllDatUpdates);
        }
        if view.update_health.is_empty() {
            ui.label("No installed catalogues.");
        }
        egui::CollapsingHeader::new("Read-only update-all plan").default_open(true).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("dat-health-overview").max_height(340.0).show(ui, |ui| {
                for (index, row) in view.update_health.iter().enumerate() {
                    ui.push_id(index, |ui| {
                        ui.separator();
                        ui.strong(format!("{} · {}", row.name, row.assessment.health.label()));
                        ui.label(format!("Installed: {} · Available: {} · Last checked: {}",
                            row.installed_version.as_deref().unwrap_or("Unknown"),
                            row.assessment.available_version.as_deref().unwrap_or("Unknown"),
                            row.last_checked_display.as_deref().unwrap_or("Never")));
                        ui.label(match row.plan {
                            PlanDisposition::Eligible => "Plan: eligible for a separately reviewed source update",
                            PlanDisposition::Current => "Plan: no update indicated by the latest check",
                            PlanDisposition::CheckFirst => "Plan: check again before updating",
                            PlanDisposition::Unsupported => "Plan: unsupported automatic provider; manual action required",
                            PlanDisposition::Conflict => "Plan: conflict; review required",
                        });
                        ui.weak(&row.explanation);
                        ui.weak(&row.assessment.detail);
                    });
                }
            });
        });
    });
    action
}

#[cfg(test)]
mod tests;
