//! Bounded local setup list. Compatibility comes from recorded platform data;
//! unknown sources may be explicitly previewed, never relabelled for this page.
use super::*;
use crate::dat_catalogue_picker::listing::{Group, Row, State};
use std::hash::{Hash, Hasher};

#[derive(Default)]
pub(super) struct CatalogueList {
    key: Option<u64>,
    pub(super) list: State,
    providers: Vec<String>,
}
impl CatalogueList {
    fn sync(
        &mut self,
        view: &DatSourcesPageView,
        summaries: &[InstalledCatalogueSummary],
        platform: &str,
        revision: u64,
    ) {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        platform.hash(&mut hash);
        revision.hash(&mut hash);
        for row in &view.rows {
            (
                &row.id,
                &row.display_name,
                &row.path,
                &row.platform_id,
                &row.platform_display,
            )
                .hash(&mut hash);
            (
                row.enabled,
                row.health_stale,
                row.incomplete_load,
                row.entry_count,
                matches!(
                    row.health_state,
                    DatHealthState::Valid | DatHealthState::ValidWithWarnings
                ),
                row.detail
                    .as_ref()
                    .map(|detail| detail.duplicate_identities.len()),
            )
                .hash(&mut hash);
        }
        let key = hash.finish();
        if self.key == Some(key) {
            return;
        }
        let providers: std::collections::HashMap<_, _> = summaries
            .iter()
            .filter_map(|summary| match &summary.reference {
                CatalogueRef::Local { source_id, .. } => Some((
                    source_id.as_str(),
                    crate::dat_catalogue_picker::provenance_label(&summary.provenance),
                )),
                _ => None,
            })
            .collect();
        self.providers = view
            .rows
            .iter()
            .map(|row| {
                providers
                    .get(row.id.as_str())
                    .cloned()
                    .unwrap_or_else(|| "Local registry · User registered".into())
            })
            .collect();
        self.list.replace(
            view.rows
                .iter()
                .enumerate()
                .map(|(source_index, row)| {
                    let known = row
                        .platform_id
                        .as_deref()
                        .and_then(archivefs_core::canonical_platform_for_alias);
                    let group = if !row.enabled {
                        Group::Disabled
                    } else if row.platform_id.is_some() && known != Some(platform) {
                        Group::Incompatible
                    } else if !row_metadata_ready(row) {
                        Group::NeedsChecking
                    } else if known.is_none() {
                        Group::Unassigned
                    } else {
                        Group::Compatible
                    };
                    Row {
                        source_index,
                        name: row.display_name.clone(),
                        search: format!(
                            "{} {} {} {}",
                            row.display_name,
                            row.platform_id.as_deref().unwrap_or(""),
                            row.platform_display.as_deref().unwrap_or(""),
                            self.providers[source_index]
                        )
                        .to_lowercase(),
                        group,
                    }
                })
                .collect(),
        );
        self.key = Some(key);
    }
}

pub(super) fn show(
    ui: &mut egui::Ui,
    view: &DatSourcesPageView,
    state: &mut SimpleCheckUi,
    platform: &str,
    summaries: &[InstalledCatalogueSummary],
    revision: u64,
) -> Option<DatSourcesPageAction> {
    state
        .catalogue_list
        .sync(view, summaries, platform, revision);
    let mut action = None;
    ui.strong("Installed catalogues");
    ui.label("Installed does not mean compatible, selected or verified. Unknown platforms require a compatibility preview; filenames and provider names are not proof.");
    if ui.button("Refresh catalogue list").clicked() {
        state.catalogue_list.key = None;
        ui.ctx().request_repaint();
    }
    state.catalogue_list.list.controls(ui);
    let indices = state.catalogue_list.list.visible().to_vec();
    if indices.is_empty() {
        ui.label("No catalogues match these filters. Check the other groups or clear the filters.");
    }
    egui::ScrollArea::vertical()
        .id_salt(("local-catalogue-list", platform))
        .max_height(320.0)
        .show(ui, |ui| {
            for index in indices {
                let row = &view.rows[index];
                ui.push_id(("setup-row", &row.id), |ui| {
                    ui.strong(&row.display_name);
                    ui.label(format!(
                        "{} · {}",
                        row.platform_display
                            .as_deref()
                            .unwrap_or("Platform not established"),
                        state.catalogue_list.providers[index]
                    ));
                    let compatible = row
                        .platform_id
                        .as_deref()
                        .and_then(archivefs_core::canonical_platform_for_alias)
                        == Some(platform);
                    let unknown = row.platform_id.is_none();
                    if !row.enabled {
                        ui.label(
                            "Disabled — enable this source in DAT management before selecting it.",
                        );
                    } else if !compatible && !unknown {
                        ui.label("The recorded platform differs or is not recognised here. Review its assignment in DAT management before selecting it for this platform.");
                    } else if !row_metadata_ready(row) {
                        ui.label("Needs validation or refreshed evidence before use.");
                        if ui
                            .add_enabled(
                                !view.background_busy,
                                egui::Button::new("Check this verification data"),
                            )
                            .clicked()
                        {
                            action = Some(DatSourcesPageAction::Validate { id: row.id.clone() });
                        }
                    } else if ui
                        .add_enabled(
                            !view.background_busy,
                            egui::Button::new(if compatible {
                                "Select catalogue"
                            } else {
                                "Select for compatibility preview"
                            }),
                        )
                        .clicked()
                    {
                        state
                            .selected
                            .insert(platform.to_owned(), CatalogueRef::local(&row.id));
                        state.result_for = None;
                    }
                    ui.separator();
                });
            }
        });
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_list_keeps_unknown_other_disabled_and_stale_sources_separate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.dat");
        std::fs::write(&path, b"synthetic").unwrap();
        let page = DatSourcesPageState::load_with_transaction_dir_and_managed_paths(
            dir.path().join("registry.toml"),
            vec![],
            TrustedRoots::none(),
            dir.path().join("journals"),
            dir.path().join("managed.toml"),
            dir.path().join("managed"),
        );
        let mut view = page.view();
        view.rows = (0..5)
            .map(|i| DatSourceRowView {
                id: format!("s{i}"),
                display_name: format!("Catalogue {i}"),
                path: path.display().to_string(),
                enabled: true,
                health_state: DatHealthState::Valid,
                entry_count: Some(1),
                ..Default::default()
            })
            .collect();
        view.rows[0].platform_id = Some("Amiga".into());
        view.rows[1].platform_id = Some("NES".into());
        view.rows[3].enabled = false;
        view.rows[4].platform_id = Some("Amiga".into());
        view.rows[4].health_stale = true;
        let mut cache = CatalogueList::default();
        cache.sync(&view, &[], "Amiga", 1);
        for (group, index) in [
            (Group::Compatible, 0),
            (Group::Incompatible, 1),
            (Group::Unassigned, 2),
            (Group::Disabled, 3),
            (Group::NeedsChecking, 4),
        ] {
            cache.list.group = group;
            cache.list.refilter();
            assert_eq!(cache.list.visible(), &[index]);
        }
        view.rows[0].enabled = false;
        cache.sync(&view, &[], "Amiga", 1);
        cache.list.group = Group::Compatible;
        cache.list.refilter();
        assert!(cache.list.visible().is_empty());
    }
}

#[cfg(test)]
mod unresolved_assignment_tests {
    use super::*;
    #[test]
    fn an_unrecognised_explicit_assignment_is_not_presented_as_unassigned_or_compatible() {
        let dir = tempfile::tempdir().unwrap();
        let page = DatSourcesPageState::load_with_transaction_dir_and_managed_paths(
            dir.path().join("registry.toml"),
            vec![],
            TrustedRoots::none(),
            dir.path().join("journals"),
            dir.path().join("managed.toml"),
            dir.path().join("managed"),
        );
        let mut view = page.view();
        view.rows = vec![DatSourceRowView {
            id: "future".into(),
            display_name: "Future catalogue".into(),
            platform_id: Some("FuturePlatformNotKnown".into()),
            enabled: true,
            health_state: DatHealthState::Valid,
            entry_count: Some(1),
            ..Default::default()
        }];
        let mut cache = CatalogueList::default();
        cache.sync(&view, &[], "Amiga", 1);
        cache.list.refilter();
        assert!(cache.list.visible().is_empty());
        cache.list.group = Group::Unassigned;
        cache.list.refilter();
        assert!(cache.list.visible().is_empty());
        cache.list.group = Group::Incompatible;
        cache.list.refilter();
        assert_eq!(cache.list.visible(), &[0]);
    }
}
