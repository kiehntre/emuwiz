use super::*;
use archivefs_core::dat::catalogue_selection::CatalogueCapabilities;
use std::time::Instant;

pub(super) fn summary(index: usize) -> InstalledCatalogueSummary {
    InstalledCatalogueSummary {
        reference: CatalogueRef::local(format!("source-{index:05}")),
        store: CatalogueStore::LocalRegistry,
        display_name: format!("Catalogue {index:05}"),
        platform: EvidenceValue::Confirmed("Amiga".into()),
        ecosystem: EvidenceValue::Unknown,
        variant: EvidenceValue::Unknown,
        revision: None,
        imported_or_retrieved_at: None,
        content_sha256: None,
        provenance: CatalogueProvenance::UserRegistered,
        availability: CatalogueAvailability::Ready,
        enabled: true,
        capabilities: CatalogueCapabilities {
            verify: true,
            combined_evidence: true,
            repair: true,
            single_catalogue_1g1r: true,
        },
        technical_path: None,
    }
}

#[test]
#[ignore = "bounded synthetic DAT selector performance measurement"]
fn selector_profile_shared_picker() {
    for count in [456_usize, 5000] {
        let mut picker = DatCataloguePickerState {
            loaded: true,
            summaries: (0..count).map(summary).collect(),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let mut selected = None;
        let mut times = Vec::new();
        for frame in 0..8 {
            let start = Instant::now();
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1100.0, 720.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let _ = picker.show(ui, DatCatalogueWorkflow::Verify, &mut selected);
                    });
                },
            );
            if frame > 0 {
                times.push(start.elapsed().as_secs_f64() * 1000.0);
            }
        }
        times.sort_by(f64::total_cmp);
        println!(
            "SHARED_PICKER_PROFILE count={count} render_p50_ms={:.3} render_max_ms={:.3}",
            times[times.len() / 2],
            times.last().unwrap()
        );
        assert!(selected.is_none());
    }
}

#[test]
fn disabled_and_unsupported_sources_are_not_actionable_and_filtering_never_selects() {
    let mut row = summary(0);
    assert!(summary_usable(DatCatalogueWorkflow::Verify, &row));
    row.enabled = false;
    assert!(!summary_usable(DatCatalogueWorkflow::Verify, &row));
    row.enabled = true;
    row.capabilities.verify = false;
    assert!(!summary_usable(DatCatalogueWorkflow::Verify, &row));
    row.capabilities.verify = true;
    row.availability = CatalogueAvailability::StaleManagedState {
        reason: "changed".into(),
    };
    assert!(!summary_usable(DatCatalogueWorkflow::Verify, &row));
    row.availability = CatalogueAvailability::NeedsValidation {
        reason: "preview must validate".into(),
    };
    assert!(summary_usable(DatCatalogueWorkflow::Verify, &row));
    assert!(!summary_usable(DatCatalogueWorkflow::PlayingLibrary, &row));
}

#[test]
fn shared_picker_renders_at_most_one_page_and_never_selects_a_filtered_row_implicitly() {
    let mut picker = DatCataloguePickerState {
        loaded: true,
        summaries: (0..5000).map(summary).collect(),
        ..Default::default()
    };
    let ctx = egui::Context::default();
    let mut selected = None;
    let _ = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            assert!(
                picker
                    .show(ui, DatCatalogueWorkflow::Verify, &mut selected)
                    .is_none()
            );
        });
    });
    assert_eq!(picker.listing.visible().len(), listing::PAGE_SIZE);
    assert!(selected.is_none());
    selected = Some(CatalogueRef::local("source-04999"));
    picker.listing.query = "not present".into();
    let _ = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            assert!(
                picker
                    .show(ui, DatCatalogueWorkflow::Verify, &mut selected)
                    .is_none()
            );
        });
    });
    assert!(picker.listing.visible().is_empty());
    assert_eq!(selected, Some(CatalogueRef::local("source-04999")));
}

#[test]
fn publishing_an_inventory_result_changes_the_search_projection_revision() {
    let dir = tempfile::tempdir().unwrap();
    let (sender, receiver) = mpsc::channel();
    let mut picker = DatCataloguePickerState {
        loading: true,
        generation: 7,
        receiver: Some(receiver),
        ..Default::default()
    };
    let before = picker.revision();
    sender
        .send(InventoryMessage::Loaded(
            7,
            Ok(LoadedInventory {
                summaries: vec![summary(0)],
                snapshot: CatalogueInventorySnapshot {
                    local_registry: DatSourceRegistry::new(),
                    managed_sources: ManagedDatSources::new(),
                    managed_root: dir.path().into(),
                },
            }),
        ))
        .unwrap();
    assert!(picker.poll());
    assert_ne!(picker.revision(), before);
    assert_eq!(picker.summaries().len(), 1);
}
