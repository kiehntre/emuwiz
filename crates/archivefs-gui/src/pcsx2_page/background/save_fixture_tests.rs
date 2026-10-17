//! Reuse the backend's synthetic card layout, never production save data.
use super::super::*;
use archivefs_core::memory_card_inventory::*;
const PS2_PAGE_DATA_BYTES: usize = 512;
const PS2_DIRECTORY_ENTRY_BYTES: usize = 512;
fn ps2_fixture(page_stride: usize) -> Vec<u8> {
    let clusters = 8192usize;
    let pages_per_cluster = 2usize;
    let mut card = vec![0u8; clusters * pages_per_cluster * page_stride];
    card[..28].copy_from_slice(b"Sony PS2 Memory Card Format ");
    card[0x1c..0x24].copy_from_slice(b"1.2.0.0\0");
    card[0x28..0x2a].copy_from_slice(&(PS2_PAGE_DATA_BYTES as u16).to_le_bytes());
    card[0x2a..0x2c].copy_from_slice(&(pages_per_cluster as u16).to_le_bytes());
    card[0x2c..0x2e].copy_from_slice(&(16u16).to_le_bytes());
    card[0x30..0x34].copy_from_slice(&(clusters as u32).to_le_bytes());
    card[0x34..0x38].copy_from_slice(&(41u32).to_le_bytes());
    card[0x38..0x3c].copy_from_slice(&(8135u32).to_le_bytes());
    card[0x3c..0x40].copy_from_slice(&(0u32).to_le_bytes());
    card[0x40..0x44].copy_from_slice(&(1023u32).to_le_bytes());
    card[0x44..0x48].copy_from_slice(&(1022u32).to_le_bytes());
    card[0x50..0x54].copy_from_slice(&7u32.to_le_bytes());
    // Actual two-level layout: superblock -> IFC cluster 7 -> FAT tables 8..39.
    for table in 0..32usize {
        let offset = 7 * pages_per_cluster * page_stride + table * 4;
        card[offset..offset + 4].copy_from_slice(&(8u32 + table as u32).to_le_bytes());
    }
    let fat_cluster_offset = 8 * pages_per_cluster * page_stride;
    card[fat_cluster_offset..fat_cluster_offset + 4].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
    card
}

fn ps2_inventory_fixture() -> Vec<u8> {
    let mut card = ps2_fixture(PS2_PAGE_DATA_BYTES);
    card[0x50..0x54].copy_from_slice(&7u32.to_le_bytes());
    card[8 * 1024..8 * 1024 + 4].copy_from_slice(&0x8000_0009u32.to_le_bytes());
    for relative in [3usize, 4, 9] {
        let offset = 8 * 1024 + relative * 4;
        card[offset..offset + 4].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
    }
    card[8 * 1024 + 2 * 4..8 * 1024 + 2 * 4 + 4].copy_from_slice(&0x8000_0004u32.to_le_bytes());

    fn entry(
        card: &mut [u8],
        relative: usize,
        slot: usize,
        mode: u32,
        length: u32,
        cluster: u32,
        name: &[u8],
    ) {
        let offset = (41 + relative) * 1024 + slot * PS2_DIRECTORY_ENTRY_BYTES;
        card[offset..offset + 4].copy_from_slice(&mode.to_le_bytes());
        card[offset + 4..offset + 8].copy_from_slice(&length.to_le_bytes());
        card[offset + 0x08..offset + 0x10].copy_from_slice(&[0, 38, 44, 12, 24, 6, 0xea, 0x07]);
        card[offset + 0x10..offset + 0x14].copy_from_slice(&cluster.to_le_bytes());
        card[offset + 0x18..offset + 0x20].copy_from_slice(&[0, 39, 44, 12, 24, 6, 0xea, 0x07]);
        card[offset + 0x40..offset + 0x40 + name.len()].copy_from_slice(name);
    }

    entry(&mut card, 0, 0, 0x8427, 3, 0, b".");
    entry(&mut card, 0, 1, 0xa426, 0, 0, b"..");
    entry(&mut card, 9, 0, 0x8427, 3, 2, b"BASLUS-00001TEST");
    entry(&mut card, 2, 0, 0x8427, 3, 2, b".");
    entry(&mut card, 2, 1, 0xa426, 0, 0, b"..");
    entry(&mut card, 4, 0, 0x8497, 100, 3, b"icon.sys");
    card[(41 + 3) * 1024..(41 + 3) * 1024 + 100].fill(0x5a);
    card
}

fn fixture() -> (tempfile::TempDir, MemoryCardInventory, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("synthetic.ps2");
    std::fs::write(&card, ps2_inventory_fixture()).unwrap();
    let inventory = inspect_memory_card(&card).unwrap();
    let save = &inventory.ps2_inventory.as_ref().unwrap().save_directories[0];
    let psu = dir.path().join("synthetic.psu");
    apply_ps2_psu_export(&plan_ps2_psu_export(&inventory, save, &psu).unwrap()).unwrap();
    let backup = dir.path().join("backup.ps2");
    std::fs::create_dir(dir.path().join("records")).unwrap();
    (dir, inventory, psu, backup)
}
fn render(ctx: &egui::Context) -> Vec<String> {
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| show_psu_restore_dialog(ui, true));
    });
    ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| show_psu_restore_dialog(ui, true));
    })
    .shapes
    .iter()
    .filter_map(|s| match &s.shape {
        egui::Shape::Text(t) => Some(t.galley.job.text.clone()),
        _ => None,
    })
    .collect()
}
#[test]
fn a_real_replacement_preview_preserves_source_metadata_and_requires_confirmation() {
    let (_dir, card, psu, backup) = fixture();
    let original = std::fs::read(&card.path).unwrap();
    assert!(matches!(
        plan_ps2_psu_restore(&card, &psu, &backup, false),
        Err(Ps2PsuRestoreError::ExistingSave(_))
    ));
    let plan = plan_ps2_psu_restore(&card, &psu, &backup, true).unwrap();
    assert!(plan.replace_existing && plan.existing_save);
    let expected_name = plan.save_display_name.clone();
    let ctx = egui::Context::default();
    ctx.data_mut(|d| {
        d.insert_temp(
            psu_restore_dialog_id(),
            PsuRestoreDialogState::Confirm(Box::new(plan)),
        )
    });
    let shown = render(&ctx);
    assert!(shown.iter().any(|s| s.contains("Preview PS2 save restore")));
    assert!(shown.iter().any(|s| s.contains(&expected_name)));
    assert!(
        shown
            .iter()
            .any(|s| s.contains("replace the existing save"))
    );
    assert!(
        shown
            .iter()
            .any(|s| s.contains("Create backup and restore"))
    );
    assert_eq!(std::fs::read(&card.path).unwrap(), original);
    assert!(!backup.exists());
    assert!(!super::busy(&ctx));
}
struct Closed;
impl archivefs_core::save_snapshots::tree_restore::safety::QuiescenceProvider for Closed {
    fn observe(
        &self,
        _: &archivefs_core::save_snapshots::tree_restore::safety::DirectorySaveBinding,
    ) -> archivefs_core::save_snapshots::tree_restore::safety::EmulatorQuiescence {
        archivefs_core::save_snapshots::tree_restore::safety::EmulatorQuiescence::Closed
    }
}
fn finish(ctx: &egui::Context) {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while super::busy(ctx) {
        super::poll(ctx);
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}
#[test]
fn verified_guarded_restore_and_exact_undo_results_cross_the_gui_worker_boundary() {
    let (dir, card, psu, backup) = fixture();
    let original = std::fs::read(&card.path).unwrap();
    let plan = plan_ps2_psu_restore(&card, &psu, &backup, true).unwrap();
    let options = archivefs_core::memory_card_inventory::restore_guard::Ps2RestoreGuardOptions {
        journal_dir: dir.path().join("records"),
        unix_seconds: 1700000000,
    };
    let ctx = egui::Context::default();
    assert!(super::start(
        &ctx,
        "synthetic confirmed restore",
        move || super::Outcome::Restore(
            match apply_ps2_psu_restore_guarded(&plan, &Closed, &options) {
                Ok(done) => PsuRestoreDialogState::Success(Box::new(done)),
                Err(e) => panic!("synthetic guarded restore failed: {e:?}"),
            }
        )
    ));
    finish(&ctx);
    let done = ctx.data(|d| d.get_temp::<PsuRestoreDialogState>(psu_restore_dialog_id()));
    let Some(PsuRestoreDialogState::Success(done)) = done else {
        panic!("not verified success")
    };
    assert_eq!(std::fs::read(&backup).unwrap(), original);
    assert!(
        render(&ctx)
            .iter()
            .any(|s| s.contains("Restore completed and verified"))
    );
    let journal = done.journal_path.clone();
    assert!(super::start(&ctx, "synthetic confirmed Undo", move || {
        super::Outcome::Restore(
            match undo_ps2_psu_restore_guarded(&journal, &Closed, 1700000001) {
                Ok(()) => PsuRestoreDialogState::Undone,
                Err(e) => panic!("synthetic Undo failed: {e:?}"),
            },
        )
    }));
    finish(&ctx);
    assert!(render(&ctx).iter().any(|s| s == "Restore undone"));
    assert_eq!(std::fs::read(&card.path).unwrap(), original);
}
#[test]
fn stale_card_is_a_truthful_failure_and_does_not_create_a_backup() {
    let (dir, card, psu, backup) = fixture();
    let plan = plan_ps2_psu_restore(&card, &psu, &backup, true).unwrap();
    let changed = b"synthetic external replacement";
    std::fs::write(&card.path, changed).unwrap();
    let options = archivefs_core::memory_card_inventory::restore_guard::Ps2RestoreGuardOptions {
        journal_dir: dir.path().join("records"),
        unix_seconds: 1700000000,
    };
    let ctx = egui::Context::default();
    assert!(super::start(&ctx, "synthetic stale restore", move || {
        super::Outcome::Restore(
            match apply_ps2_psu_restore_guarded(&plan, &Closed, &options) {
                Ok(_) => panic!("stale restore accepted"),
                Err(e) => PsuRestoreDialogState::Refused(psu_restore_error_message(&e)),
            },
        )
    }));
    finish(&ctx);
    assert!(render(&ctx).iter().any(|s| s.contains("Restore refused")));
    assert!(!render(&ctx).iter().any(|s| s.contains("Restore completed")));
    assert!(!backup.exists());
    assert_eq!(std::fs::read(&card.path).unwrap(), changed);
}
