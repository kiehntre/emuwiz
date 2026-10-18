//! Disposable real LHA fixture through Blue's existing native automatic journey.
use super::super::native_import_journey::Journey;
use super::*;
use archivefs_core::ingestion::bounded_import::apply_bounded_import;

fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_be_bytes());
}
fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_be_bytes());
}
fn minimal_whdload_slave() -> Vec<u8> {
    let size: usize = 30;
    let mut code = vec![0; (size + 64).next_multiple_of(4)];
    code[..4].copy_from_slice(&[0x70, 0xff, 0x4e, 0x75]);
    code[4..12].copy_from_slice(b"WHDLOADS");
    put16(&mut code, 12, 1); // runtime_version
    put16(&mut code, 14, 3);
    put32(&mut code, 16, 524288);
    put32(&mut code, 20, 1);
    put16(&mut code, 24, (size + 40) as u16);
    put16(&mut code, 26, 0);
    put16(&mut code, 28, 0);
    code[size..size + 5].copy_from_slice(b"Game\0");
    code[size + 8..size + 13].copy_from_slice(b"Copy\0");
    code[size + 16..size + 21].copy_from_slice(b"Info\0");
    code[size + 24..size + 29].copy_from_slice(b"Kick\0");
    code[size + 32..size + 39].copy_from_slice(b"Config\0");
    let mut out = Vec::new();
    for n in [
        0x3f3_u32,
        0,
        1,
        0,
        0,
        (code.len() / 4) as u32,
        0x3e9,
        (code.len() / 4) as u32,
    ] {
        out.extend_from_slice(&n.to_be_bytes());
    }
    out.extend_from_slice(&code);
    out.extend_from_slice(&0x3f2_u32.to_be_bytes());
    out
}

fn lha_entry(name: &str, payload: &[u8]) -> Vec<u8> {
    let header_size = name.len() + 23;
    let mut bytes = vec![header_size as u8, 0];
    bytes.extend_from_slice(b"-lh0-");
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.push(0x20);
    bytes.push(0);
    bytes.push(name.len() as u8);
    bytes.extend_from_slice(name.as_bytes());
    bytes.extend_from_slice(&lha_crc16(payload).to_le_bytes());
    bytes.push(0);
    bytes[1] = bytes[2..]
        .iter()
        .fold(0_u8, |sum, byte| sum.wrapping_add(*byte));
    bytes.extend_from_slice(payload);
    bytes
}

fn stored_lha(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (name, payload) in entries {
        bytes.extend(lha_entry(name, payload));
    }
    bytes.push(0);
    bytes
}

fn lha_crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0_u16;
    for byte in bytes {
        crc ^= u16::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xa001
            } else {
                crc >> 1
            };
        }
    }
    crc
}

#[test]
fn amiga_lha_blue_automatic_review_import_receipt_and_undo_preserve_sources() {
    assert!(
        archivefs_core::dat::archive::lha::LhaProvider::discover(std::time::Duration::from_secs(5))
            .is_ok(),
        "Focused LHA regression needs the installed decoder"
    );
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Amiga");
    std::fs::create_dir(&root).unwrap();
    let database = temp.path().join("library.sqlite3");
    let cfg = temp.path().join("config.toml");
    archivefs_core::add_source_folder_at(&cfg, &database, &root).unwrap();
    let mut db = Database::open_or_create(&database).unwrap();
    let source = db.list_source_folders().unwrap().remove(0);
    let review = db.review_source_rebind(source.id).unwrap();
    db.confirm_source_rebind(&review).unwrap();
    let sources = archivefs_core::build_source_folder_views(
        &archivefs_core::load_source_folder_configs_from(&cfg).unwrap(),
        &db.list_source_folders().unwrap(),
    );
    let slave = minimal_whdload_slave();
    let bytes = stored_lha(&[
        ("Game/Game.slave", &slave),
        ("Game/disk.1", b"synthetic game resource"),
        ("Game/ReadMe.txt", b"synthetic documentation"),
        ("Game/empty", b""),
    ]);
    let path = root.join("Synthetic.lha");
    std::fs::write(&path, &bytes).unwrap();
    let ambiguous = root.join("Ambiguous.lha");
    let ambiguous_bytes = stored_lha(&[("A.slave", &slave), ("B.slave", &slave)]);
    std::fs::write(&ambiguous, &ambiguous_bytes).unwrap();
    let before = std::fs::read(&database).unwrap();
    let report = run_at(
        root.clone(),
        database.clone(),
        sources,
        &cfg,
        &AtomicBool::new(false),
        &mut |_, _| {},
    )
    .unwrap();
    let plan = report.bounded_import.as_ref().unwrap();
    assert_eq!(plan.projected_insertions(), 1);
    assert_eq!(plan.platform_counts().get("Amiga"), Some(&1));
    let refusal = report.items.iter().find(|r| r.path == ambiguous).unwrap();
    assert_eq!(refusal.disposition, MediaDisposition::Excluded);
    assert!(
        refusal
            .explanation
            .contains("Multiple WHDLoad slaves remain ambiguous"),
        "{}",
        refusal.explanation
    );
    let ctx = egui::Context::default();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 600.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show_report(ui, &report, &mut 5, &mut 0));
        },
    );
    fn contains_reason(shape: &egui::Shape) -> bool {
        match shape {
            egui::Shape::Text(t) => t
                .galley
                .text()
                .contains("Multiple WHDLoad slaves remain ambiguous"),
            egui::Shape::Vec(shapes) => shapes.iter().any(contains_reason),
            _ => false,
        }
    }
    assert!(
        output.shapes.iter().any(|s| contains_reason(&s.shape)),
        "Blue's unsupported-package row must show its actual reason"
    );
    let mut journey = Journey::new(&report, plan).unwrap();
    let batch = journey
        .prepare_first(&database, &cfg, &AtomicBool::new(false), &mut |_, _| {})
        .unwrap()
        .unwrap();
    assert_eq!(batch.plan.projected_insertions(), 1);
    assert!(batch.rejected.is_empty());
    assert_eq!(
        std::fs::read(&database).unwrap(),
        before,
        "discovery and batch review grant no automatic write authority"
    );
    assert!(db.load_archives().unwrap().is_empty());
    // Explicit fixture approval of this exact sealed batch, using the existing importer.
    let result =
        apply_bounded_import(&batch.plan, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert_eq!(result.inserted.len(), 1);
    let operation = result.operation_id.unwrap();
    let history = db.discovery_import_history(0, 50).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].operation_id, operation);
    assert!(!history[0].approval_sha256.is_empty());
    let row = db.load_archives().unwrap().remove(0);
    assert_eq!(row.absolute_path, path);
    assert_eq!(row.platform.as_deref(), Some("Amiga"));
    assert_eq!(row.archive_kind, "lha");
    assert!(apply_bounded_import(&batch.plan, &AtomicBool::new(false), &mut |_, _| {}).is_err());
    let undo = db.preview_discovery_import_undo(&operation).unwrap();
    assert!(undo.can_undo(), "{:?}", undo.blockers);
    assert_eq!(
        Database::undo_discovery_import(&undo)
            .unwrap()
            .removed_records,
        1
    );
    assert!(db.load_archives().unwrap().is_empty());
    assert!(db.discovery_import_history(0, 50).unwrap()[0].undone);
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    assert_eq!(std::fs::read(ambiguous).unwrap(), ambiguous_bytes);
}
