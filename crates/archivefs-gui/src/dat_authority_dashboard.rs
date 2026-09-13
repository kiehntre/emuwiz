//! Cached, asynchronous read-only DAT dashboard. The UI never opens ROMs,
//! parses DATs, chooses authority automatically or invokes an apply action.
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Instant;

use archivefs_core::Database;
use archivefs_core::dat::authority::*;
use eframe::egui;

type ReadResult = Result<DatAuthorityDashboard, String>;
type DiffResult = Result<DatRefreshImpact, String>;

#[derive(Default)]
pub(crate) struct DashboardState {
    pub(crate) data: Option<ReadResult>,
    reader: Option<Receiver<(u64, ReadResult, f64)>>,
    comparison_reader: Option<Receiver<(u64, DiffResult)>>,
    generation: Option<u64>,
    token: u64,
    requested: bool,
    elapsed_ms: Option<f64>,
    filter: String,
    page: usize,
    tab: usize,
    old: String,
    new: String,
    comparison: Option<DiffResult>,
    same_catalogue: bool,
}

impl DashboardState {
    pub(crate) fn is_busy(&self) -> bool {
        self.reader.is_some() || self.comparison_reader.is_some()
    }

    pub(crate) fn invalidate(&mut self) {
        self.token = self.token.wrapping_add(1);
        self.data = None;
        self.comparison = None;
        self.requested = false;
    }

    /// Returns true when attention projection must be rebuilt. At most one
    /// reader per state; stale worker generations are discarded, never shown.
    pub(crate) fn tick(
        &mut self,
        generation: u64,
        path: Option<PathBuf>,
        active: bool,
        ctx: &egui::Context,
    ) -> bool {
        let mut changed = false;
        if self.generation != Some(generation) {
            self.generation = Some(generation);
            self.invalidate();
            changed = true;
        }
        if let Some(rx) = &self.reader {
            match rx.try_recv() {
                Ok((token, data, elapsed)) => {
                    self.reader = None;
                    if token == self.token {
                        self.data = Some(data);
                        self.elapsed_ms = Some(elapsed);
                        changed = true;
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    self.reader = None;
                    self.data = Some(Err("DAT dashboard reader stopped; refresh to retry.".into()));
                    changed = true;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(rx) = &self.comparison_reader {
            match rx.try_recv() {
                Ok((token, result)) => {
                    if token == self.token {
                        self.comparison = Some(result);
                    }
                    self.comparison_reader = None;
                }
                Err(TryRecvError::Disconnected) => {
                    self.comparison = Some(Err("Comparison reader stopped.".into()));
                    self.comparison_reader = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if active && !self.requested && self.reader.is_none() && self.comparison_reader.is_none() {
            self.requested = true;
            let (tx, rx) = mpsc::channel();
            self.reader = Some(rx);
            let token = self.token;
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let start = Instant::now();
                let result = load(path).map_err(|e| e.to_string());
                // load has returned and its database connection is CLOSED
                // before signalling completion (restore safety boundary).
                let _ = tx.send((token, result, start.elapsed().as_secs_f64() * 1000.0));
                ctx.request_repaint();
            });
        }
        changed
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, database_path: Option<PathBuf>) -> bool {
        let mut refresh = false;
        ui.heading("DAT authority & collection completeness");
        ui.label(
            "Read-only • recorded catalogue evidence • no downloads, renames or automatic fixes",
        );
        ui.horizontal(|ui| {
            refresh = ui
                .add_enabled(!self.is_busy(), egui::Button::new("Refresh dashboard"))
                .clicked();
            if let Some(ms) = self.elapsed_ms {
                ui.label(format!("Last database/config read: {ms:.0} ms"));
            }
        });
        if refresh {
            self.invalidate();
        }
        let Some(data) = &self.data else {
            ui.label("Reading catalogue evidence… No filesystem scan is performed.");
            return refresh;
        };
        let data = match data {
            Ok(d) => d,
            Err(e) => {
                ui.label(format!("Dashboard unavailable: {e}"));
                return refresh;
            }
        };
        for warning in &data.warnings {
            ui.label(warning);
        }
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.tab, 0, "By platform");
            ui.selectable_value(&mut self.tab, 1, "Authority preparation");
            ui.selectable_value(&mut self.tab, 2, "Refresh impact (read-only)");
        });
        ui.separator();
        if self.tab == 0 {
            ui.label(format!("{} platform/source rows. Overlapping DATs are shown separately, never added together.",data.collections.len()));
            ui.horizontal(|ui| {
                ui.label("Platform filter");
                if ui.text_edit_singleline(&mut self.filter).changed() {
                    self.page = 0;
                }
            });
            let filtered = filtered_rows(data, &self.filter);
            let pages = filtered.len().div_ceil(50).max(1);
            self.page = self.page.min(pages - 1);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(self.page > 0, egui::Button::new("Previous"))
                    .clicked()
                {
                    self.page -= 1;
                }
                ui.label(format!("Page {} of {pages}", self.page + 1));
                if ui
                    .add_enabled(self.page + 1 < pages, egui::Button::new("Next"))
                    .clicked()
                {
                    self.page += 1;
                }
            });
            if filtered.is_empty() {
                ui.label("No catalogued platforms match. Scan a configured source to populate the catalogue; this dashboard does not scan automatically.");
            }
            for row in filtered.into_iter().skip(self.page * 50).take(50) {
                let authority = data
                    .authorities
                    .iter()
                    .find(|a| Some(&a.source.id) == row.source_id.as_ref());
                let source = authority
                    .map(|a| a.source.name.as_str())
                    .unwrap_or("No DAT assigned");
                egui::CollapsingHeader::new(format!("{} — {} — {}", row.platform, source, row.state.label()))
                    .id_salt(("dat-authority",&row.platform,&row.source_id)).default_open(true).show(ui,|ui| {
                        let c = &row.counts;
                        ui.label(format!("Expected {}  •  Matched {}  •  Missing {}  •  Ambiguous local {}  •  Extra local {}", count(c.expected),count(c.matched),count(c.missing),c.ambiguous,c.extra));
                        ui.label(format!("Verified local {} / {}  •  Unidentified {}  •  Pending entries {}  •  Recorded BIOS gaps {}",c.verified_local,c.local,c.unidentified_local,count(c.pending_entries),count(c.bios_missing)));
                        if let Some(a) = authority { authority_details(ui,a); }
                        for explanation in &row.explanations { ui.label(explanation); }
                    });
            }
        } else if self.tab == 1 {
            for row in data
                .collections
                .iter()
                .filter(|c| c.state == CompletenessState::NoAuthority)
            {
                ui.label(format!("{}: no assigned authority. Register/link a DAT using the existing source controls below.",row.platform));
            }
            if data.authorities.is_empty() {
                ui.label("No DAT sources or inventories recorded. Add a local DAT using the source controls below.");
            }
            for a in &data.authorities {
                egui::CollapsingHeader::new(&a.source.name)
                    .id_salt(("authority-preparation", &a.source.id))
                    .show(ui, |ui| {
                        authority_details(ui, a);
                        for message in &a.preparation {
                            ui.label(message);
                        }
                    });
            }
        } else {
            ui.label("Compare two already imported inventories. Keep old/new DATs registered separately to compare them; replaced inventory generations are not retained here.");
            ui.label("Only compare the same platform/ecosystem/variant. This preview does not choose authority or apply any changes.");
            ui.checkbox(&mut self.same_catalogue, "These are revisions of the same catalogue and variant (permits shared-ID rename comparison)");
            for (label, value) in [
                ("Old authority", &mut self.old),
                ("New authority", &mut self.new),
            ] {
                egui::ComboBox::from_id_salt(label)
                    .selected_text(if value.is_empty() {
                        label
                    } else {
                        value.as_str()
                    })
                    .show_ui(ui, |ui| {
                        for a in data.authorities.iter().filter(|a| a.validated_at.is_some()) {
                            ui.selectable_value(value, a.source.id.clone(), &a.source.name);
                        }
                    });
            }
            if ui
                .add_enabled(
                    !self.is_busy() && !self.old.is_empty() && !self.new.is_empty(),
                    egui::Button::new("Compare retained inventories"),
                )
                .clicked()
            {
                let old = self.old.clone();
                let new = self.new.clone();
                let same_catalogue = self.same_catalogue;
                let token = self.token;
                let ctx = ui.ctx().clone();
                let (tx, rx) = mpsc::channel();
                self.comparison_reader = Some(rx);
                std::thread::spawn(move || {
                    let result = database_path
                        .ok_or_else(|| "No library database configured".to_string())
                        .and_then(|p| Database::open_read_only(p).map_err(|e| e.to_string()))
                        .and_then(|db| {
                            db.compare_dat_authorities(&old, &new, same_catalogue)
                                .map_err(|e| e.to_string())
                        });
                    let _ = tx.send((token, result));
                    ctx.request_repaint();
                });
            }
            if let Some(result) = &self.comparison {
                match result {
                    Ok(d) => {
                        ui.label(format!(
                            "{} → {}: {} added • {} removed • {} shared-ID renames",
                            d.old_source,
                            d.new_source,
                            d.added,
                            d.removed,
                            count(d.renamed)
                        ));
                        ui.label(
                            "Hash changes: unavailable • BIOS requirement changes: unavailable",
                        );
                        ui.label(&d.explanation);
                    }
                    Err(e) => {
                        ui.label(format!("Comparison unavailable: {e}"));
                    }
                }
            }
        }
        ui.separator();
        refresh
    }
}

fn count(n: Option<u64>) -> String {
    n.map(|n| n.to_string()).unwrap_or_else(|| "unknown".into())
}

fn filtered_rows<'a>(
    data: &'a DatAuthorityDashboard,
    filter: &str,
) -> Vec<&'a CollectionCompleteness> {
    let filter = filter.to_lowercase();
    data.collections
        .iter()
        .filter(|c| c.platform.to_lowercase().contains(&filter))
        .collect()
}

fn authority_details(ui: &mut egui::Ui, a: &DatAuthorityStatus) {
    let ecosystem = a
        .ecosystem
        .map(|e| e.label())
        .unwrap_or("Ecosystem unknown");
    ui.label(format!(
        "{ecosystem} • Variant: {} • Revision: {}",
        a.variant.as_deref().unwrap_or("not recorded"),
        a.inventory_revision.as_deref().unwrap_or("not recorded")
    ));
    ui.label(format!(
        "Source ID: {} • Imported inventory validated: {}",
        a.source.id,
        a.validated_at.as_deref().unwrap_or("not recorded")
    ));
    ui.label(format!(
        "Source SHA-256: {}",
        a.source
            .sha256
            .as_deref()
            .unwrap_or("not retained for this source")
    ));
    ui.label(&a.authority_confidence);
    if let Some(time) = &a.source.imported_at {
        ui.label(format!("Source registration/retrieval: {time}"));
    }
    ui.label(match a.freshness {
        AuthorityFreshness::Stale => "Authority changed since inventory capture; review required.",
        AuthorityFreshness::Unknown => {
            "Publisher freshness is unknown; imported does not mean latest."
        }
        AuthorityFreshness::Current => "Publisher freshness independently checked.",
    });
    ui.label(format!("Provenance: {}", a.source.provenance));
}

fn load(path: Option<PathBuf>) -> archivefs_core::Result<DatAuthorityDashboard> {
    use archivefs_core::dat::{managed_sources, sources, updates};
    let path = path.ok_or_else(|| {
        archivefs_core::ArchiveFsError::Database(
            "No library catalogue yet. Add a source and scan it first.".into(),
        )
    })?;
    let config = sources::load_dat_sources_config_default()?;
    let mut inputs = local_authorities(&config)?;
    let mut warnings = Vec::new();
    // Fixed configured state files only, never scan managed storage or rehash
    // snapshots. Managed IDs must match the existing audit source IDs.
    let managed = managed_sources::load_managed_dat_sources_default()?;
    let root = updates::managed_dat_root()?;
    for descriptor in managed.descriptors()? {
        let id = archivefs_core::dat::catalogue_selection::managed_dat_audit_source_id(
            descriptor.source_id(),
        );
        match updates::load_managed_dat_state(&root,&descriptor) {
            Ok(state) => inputs.push(DatAuthoritySource { id, name:state.authoritative_name,
                enabled:true, sha256:Some(state.sha256), ecosystem:Some(state.parsed_ecosystem),
                imported_at:state.retrieved_at_unix_seconds.map(|t|format!("Unix {t}")),
                // Upstream Git refs are not DAT header versions. Preserve
                // them as provenance, never manufacture revision drift by
                // comparing different identifier namespaces.
                provenance:format!("Managed DAT state; upstream revision {} (not a DAT header version); no explicit platform inventory assignment inferred from source name",state.upstream_revision.as_deref().unwrap_or("not recorded")), ..Default::default() }),
            Err(e) => warnings.push(format!("Managed authority {id} has no readable installed state: {e}")),
        }
    }
    let db = Database::open_read_only(path)?;
    let mut data = db.dat_authority_dashboard(&inputs)?;
    data.warnings = warnings;
    Ok(data)
}

fn local_authorities(
    config: &archivefs_core::dat::sources::config::DatSourcesConfig,
) -> archivefs_core::Result<Vec<DatAuthoritySource>> {
    use archivefs_core::dat::sources::{DatHealthState, DatSourceRegistry};
    let (registry, problems) = DatSourceRegistry::from_config(config);
    if !problems.is_empty() {
        return Err(archivefs_core::ArchiveFsError::Config(format!(
            "DAT source registry needs review: {}",
            problems.join("; ")
        )));
    }
    Ok(registry.entries().iter().map(|s| DatAuthoritySource {
        id:s.id.clone(),name:s.display_name.clone(),platform:s.platform.clone(),enabled:s.enabled,
        // Only a single recorded catalogue HEADER revision is comparable to
        // the inventory generation. Multiple/unknown revisions stay unknown.
        revision:match s.health.arcade_catalogue_revisions.as_slice() { [revision]=>revision.version.clone(),_=>None },
        ecosystem:match s.health.arcade_catalogue_revisions.as_slice() { [revision]=>Some(revision.ecosystem),_=>None },
        validation_problem:match s.health.state {
            Some(state @ (DatHealthState::Invalid | DatHealthState::Unreadable)) => Some(format!("Last source validation: {}. Review and validate the source before claiming completeness.",state.label())),
            _ => None,
        },
        imported_at:s.added_unix_seconds.map(|t|format!("Registered at Unix {t}")),
        provenance:format!("Local DAT registry: {}; {}",s.path.display(),s.origin.as_deref().unwrap_or("origin not recorded")),
        ..Default::default()
    }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_registry_reuses_header_revision_and_refuses_duplicate_ids() {
        let mut config: archivefs_core::dat::sources::config::DatSourcesConfig =
            serde_json::from_value(serde_json::json!({"sources":[{
                "id":"dat", "display_name":"Fixture DAT",
                "path":"/not-read-by-the-dashboard.dat", "kind":"file",
                "platform":"arcade", "health_arcade_catalogue_revisions":["mame_arcade=0.280"]
            }]}))
            .unwrap();
        assert_eq!(
            local_authorities(&config).unwrap()[0].revision.as_deref(),
            Some("0.280")
        );
        let sources = config.sources.as_mut().unwrap();
        sources.push(sources[0].clone());
        assert!(local_authorities(&config).is_err());
    }
    #[test]
    fn unknown_counts_are_not_zero_and_filtering_is_bounded() {
        assert_eq!(count(None), "unknown");
        let mut data = DatAuthorityDashboard::default();
        for i in 0..100_000 {
            data.collections.push(CollectionCompleteness {
                platform: format!("snes{i}"),
                source_id: None,
                state: CompletenessState::NoAuthority,
                counts: Default::default(),
                explanations: vec![],
            });
        }
        assert_eq!(filtered_rows(&data, "SNES99").len(), 1111);
        assert_eq!(
            filtered_rows(&data, "")
                .into_iter()
                .skip(50)
                .take(50)
                .count(),
            50
        );
    }
    #[test]
    fn invalidation_removes_cached_truth() {
        let mut state = DashboardState {
            data: Some(Ok(DatAuthorityDashboard::default())),
            requested: true,
            ..Default::default()
        };
        state.invalidate();
        assert!(state.data.is_none());
        assert!(!state.requested);
    }

    #[test]
    fn rendering_every_dashboard_tab_is_read_only() {
        let data = DatAuthorityDashboard {
            collections: (0..120)
                .map(|i| CollectionCompleteness {
                    platform: format!("platform-{i}"),
                    source_id: None,
                    state: CompletenessState::NoAuthority,
                    counts: CompletenessCounts {
                        local: 1000,
                        unidentified_local: 1000,
                        ..Default::default()
                    },
                    explanations: vec!["No authoritative DAT is loaded for this platform.".into()],
                })
                .collect(),
            ..Default::default()
        };
        let before = serde_json::to_value(&data).unwrap();
        let mut state = DashboardState {
            data: Some(Ok(data)),
            requested: true,
            ..Default::default()
        };
        let ctx = egui::Context::default();
        for tab in 0..3 {
            state.tab = tab;
            let start = Instant::now();
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1366.0, 768.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        assert!(!state.show(ui, None));
                    });
                },
            );
            println!(
                "DAT_DASHBOARD_CACHED_UI tab={tab} elapsed_ms={:.1}",
                start.elapsed().as_secs_f64() * 1000.0
            );
        }
        assert!(!state.is_busy());
        assert_eq!(
            before,
            serde_json::to_value(state.data.as_ref().unwrap().as_ref().unwrap()).unwrap()
        );
    }
}
