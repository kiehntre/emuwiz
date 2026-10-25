use super::*;
use crate::dreamcast_boot_evidence::ip_bin::{IpBinTextField, product_crc16};
use crate::optical_patch_tree::contract;
use crate::patch_output_recovery::tree::TreePatchState;
use crate::raw_cd_sector::SYNC_PATTERN;

fn both32(b: &mut [u8], n: u32) {
    b[..4].copy_from_slice(&n.to_le_bytes());
    b[4..8].copy_from_slice(&n.to_be_bytes());
}
fn both16(b: &mut [u8], n: u16) {
    b[..2].copy_from_slice(&n.to_le_bytes());
    b[2..4].copy_from_slice(&n.to_be_bytes());
}
fn record(name: &[u8], lba: u32, size: u32, dir: bool) -> Vec<u8> {
    let n = (33 + name.len() + 1) & !1;
    let mut b = vec![0; n];
    b[0] = n as u8;
    both32(&mut b[2..10], lba);
    both32(&mut b[10..18], size);
    b[25] = if dir { 2 } else { 0 };
    both16(&mut b[28..32], 1);
    b[32] = name.len() as u8;
    b[33..33 + name.len()].copy_from_slice(name);
    b
}
fn ip() -> Vec<u8> {
    let mut b = vec![0; IP_BIN_BYTES];
    b[..256].fill(b' ');
    b[..16].copy_from_slice(b"SEGA SEGAKATANA ");
    b[16..32].copy_from_slice(b"SEGA ENTERPRISES");
    b[32..48].copy_from_slice(b"0000 GD-ROM1/1  ");
    b[48] = b'J';
    b[56..64].copy_from_slice(b"0000000 ");
    b[64..74].copy_from_slice(b"T-1234M   ");
    b[74..80].copy_from_slice(b"V1.000");
    b[80..88].copy_from_slice(b"20000101");
    b[96..108].copy_from_slice(b"1ST_READ.BIN");
    b[128..138].copy_from_slice(b"TEST TITLE");
    let crc = format!("{:04X}", product_crc16(&b[64..80]));
    b[32..36].copy_from_slice(crc.as_bytes());
    b
}
fn iso(coordinates: ExtentCoordinates) -> Vec<u8> {
    let bias = if coordinates == ExtentCoordinates::DiscLba {
        45000
    } else {
        0
    };
    let mut b = vec![0; 32 * 2048];
    b[..IP_BIN_BYTES].copy_from_slice(&ip());
    let p = &mut b[16 * 2048..17 * 2048];
    p[0] = 1;
    p[1..6].copy_from_slice(b"CD001");
    p[6] = 1;
    both32(&mut p[80..88], bias + 32);
    both16(&mut p[120..124], 1);
    both16(&mut p[124..128], 1);
    both16(&mut p[128..132], 2048);
    both32(&mut p[132..140], 10);
    p[140..144].copy_from_slice(&(bias + 18).to_le_bytes());
    p[148..152].copy_from_slice(&(bias + 19).to_be_bytes());
    p[156..190].copy_from_slice(&record(&[0], bias + 20, 2048, true));
    let t = &mut b[17 * 2048..18 * 2048];
    t[0] = 255;
    t[1..6].copy_from_slice(b"CD001");
    t[6] = 1;
    // Single root path table, LE and BE representations.
    for (block, be) in [(18, false), (19, true)] {
        let t = &mut b[block * 2048..block * 2048 + 10];
        t[0] = 1;
        let lba = bias + 20;
        let extent = if be {
            lba.to_be_bytes()
        } else {
            lba.to_le_bytes()
        };
        t[2..6].copy_from_slice(&extent);
        let parent = if be {
            1u16.to_be_bytes()
        } else {
            1u16.to_le_bytes()
        };
        t[6..8].copy_from_slice(&parent);
    }
    let records = [
        record(&[0], bias + 20, 2048, true),
        record(&[1], bias + 20, 2048, true),
        record(b"1ST_READ.BIN;1", bias + 21, 3000, false),
        record(b"KEEP.DAT;1", bias + 23, 16, false),
    ];
    let mut cursor = 20 * 2048;
    for r in records {
        b[cursor..cursor + r.len()].copy_from_slice(&r);
        cursor += r.len();
    }
    b[21 * 2048..21 * 2048 + 3000].fill(0x41);
    b[23 * 2048..23 * 2048 + 16].fill(0x5a);
    b
}
fn raw(cooked: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    for (i, data) in cooked.chunks_exact(2048).enumerate() {
        let mut b = [0; 2352];
        b[..12].copy_from_slice(&SYNC_PATTERN);
        b[12..16].copy_from_slice(&[0x10, 0x02, i as u8, 1]);
        b[16..2064].copy_from_slice(data);
        regenerate_mode1(&mut b).unwrap();
        output.extend_from_slice(&b);
    }
    output
}
struct Fixture {
    temp: tempfile::TempDir,
    descriptor: PathBuf,
    destination: PathBuf,
    replacement: PathBuf,
    source: VerifiedGdiSource,
}
impl Fixture {
    fn new(format: SectorFormat, coordinates: ExtentCoordinates) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let low = vec![0x11; 2 * 2048];
        fs::write(source.join("track01.bin"), low).unwrap();
        fs::write(source.join("track02.raw"), vec![0x77; 3 * 2352]).unwrap();
        let iso = iso(coordinates);
        fs::write(
            source.join("track03.bin"),
            if format == SectorFormat::Cooked2048 {
                iso
            } else {
                raw(&iso)
            },
        )
        .unwrap();
        let descriptor = source.join("disc.gdi");
        fs::write(&descriptor,format!("3\n1 0 4 2048 track01.bin 0\n2 450 0 2352 track02.raw 0\n3 45000 4 {} track03.bin 0\n",format.bytes())).unwrap();
        let source = inspect_gdi_for_extent_patching(&descriptor, coordinates).unwrap();
        let replacement = temp.path().join("replace.bin");
        fs::write(&replacement, vec![0x42; 3000]).unwrap();
        let destination = temp.path().join("output");
        Self {
            temp,
            descriptor,
            destination,
            replacement,
            source,
        }
    }
    fn request(&self) -> FileReplacement {
        FileReplacement {
            filesystem_path: "1ST_READ.BIN".into(),
            expected_source_sha256: self.source.extents.0["1ST_READ.BIN"].source_sha256.clone(),
            replacement: self.replacement.clone(),
        }
    }
    fn plan(&self) -> GdiExtentPatchPlan {
        review_gdi_extent_patch(
            &self.source,
            self.source.sha256(),
            &[self.request()],
            &[],
            &self.destination,
        )
        .unwrap()
    }
}
#[test]
fn three_track_raw_replacement_topology_audio_untouched_sectors_receipt_undo() {
    let f = Fixture::new(SectorFormat::Mode1Raw2352, ExtentCoordinates::DiscLba);
    let before = contract::snapshot(f.descriptor.parent().unwrap());
    let plan = f.plan();
    assert_eq!(plan.changed_sector_count(), 2);
    let prepared = plan.prepare().unwrap();
    assert!(!f.destination.exists());
    assert_eq!(
        tree::inspect(&prepared.journal_path).unwrap(),
        TreePatchState::Staged
    );
    tree::publish(&prepared.journal_path).unwrap();
    assert_eq!(
        tree::inspect(&prepared.journal_path).unwrap(),
        TreePatchState::Published
    );
    plan.verify(&f.destination).unwrap();
    let old = fs::read(f.source.data().source_path.clone()).unwrap();
    let new = fs::read(f.destination.join("track03.bin")).unwrap();
    for i in 0..32 {
        if i != 21 && i != 22 {
            assert_eq!(old[i * 2352..(i + 1) * 2352], new[i * 2352..(i + 1) * 2352]);
        } else {
            verify_mode1(new[i * 2352..(i + 1) * 2352].try_into().unwrap()).unwrap();
        }
    }
    assert_eq!(
        fs::read(f.destination.join("track02.raw")).unwrap(),
        fs::read(f.descriptor.parent().unwrap().join("track02.raw")).unwrap()
    );
    // Tail padding of the second replaced sector remains exact too.
    assert_eq!(
        old[22 * 2352 + 16 + 952..22 * 2352 + 2064],
        new[22 * 2352 + 16 + 952..22 * 2352 + 2064]
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(f.destination.join(RECEIPT_NAME)).unwrap()).unwrap();
    assert_eq!(receipt["claim"], "EXTENT-PRESERVING VERIFIED OUTPUT");
    assert_eq!(receipt["changed_sectors"].as_array().unwrap().len(), 2);
    assert_eq!(
        receipt["output_tracks"][1]["source_sha256"],
        receipt["output_tracks"][1]["output_sha256"]
    );
    assert_eq!(
        gdi::parse_gdi_descriptor(&f.destination.join("disc.gdi"))
            .unwrap()
            .logical_topology(),
        f.source.descriptor.logical_topology()
    );
    tree::undo(&prepared.journal_path).unwrap();
    assert!(!f.destination.exists());
    assert_eq!(contract::snapshot(f.descriptor.parent().unwrap()), before);
}
#[test]
fn cooked_track_relative_same_size_path() {
    let f = Fixture::new(
        SectorFormat::Cooked2048,
        ExtentCoordinates::TrackRelativeLba,
    );
    let plan = f.plan();
    let p = plan.prepare().unwrap();
    tree::publish(&p.journal_path).unwrap();
    plan.verify(&f.destination).unwrap();
}
#[test]
fn ip_bin_edit_reuses_policy_and_sector_synthesis() {
    let f = Fixture::new(SectorFormat::Mode1Raw2352, ExtentCoordinates::DiscLba);
    let edits = [IpBinEdit::Text {
        field: IpBinTextField::Title,
        value: "PATCHED TITLE".into(),
    }];
    let p =
        review_gdi_extent_patch(&f.source, f.source.sha256(), &[], &edits, &f.destination).unwrap();
    assert_eq!(p.changed_sector_count(), 1);
    let ready = p.prepare().unwrap();
    tree::publish(&ready.journal_path).unwrap();
    p.verify(&f.destination).unwrap();
    assert_eq!(
        p.ip.as_ref()
            .unwrap()
            .ip_bin
            .as_ref()
            .unwrap()
            .metadata
            .software_title
            .value,
        "PATCHED TITLE"
    );
}
#[test]
fn wrong_source_fingerprint_and_file_hash_refused() {
    let f = Fixture::new(SectorFormat::Cooked2048, ExtentCoordinates::DiscLba);
    assert!(
        review_gdi_extent_patch(&f.source, "wrong", &[f.request()], &[], &f.destination).is_err()
    );
    let mut r = f.request();
    r.expected_source_sha256 = "wrong".into();
    assert!(
        review_gdi_extent_patch(&f.source, f.source.sha256(), &[r], &[], &f.destination).is_err()
    );
}
#[test]
fn missing_extent_and_wrong_sizes_refused() {
    let f = Fixture::new(SectorFormat::Cooked2048, ExtentCoordinates::DiscLba);
    let mut r = f.request();
    r.filesystem_path = "MISSING.BIN".into();
    assert!(
        review_gdi_extent_patch(&f.source, f.source.sha256(), &[r], &[], &f.destination).is_err()
    );
    for n in [2999, 3001] {
        fs::write(&f.replacement, vec![0; n]).unwrap();
        assert!(
            review_gdi_extent_patch(
                &f.source,
                f.source.sha256(),
                &[f.request()],
                &[],
                &f.destination
            )
            .is_err()
        );
    }
}
#[test]
fn every_source_component_or_patch_change_invalidates_preview() {
    for name in [
        "disc.gdi",
        "track01.bin",
        "track02.raw",
        "track03.bin",
        "replacement",
    ] {
        let f = Fixture::new(SectorFormat::Mode1Raw2352, ExtentCoordinates::DiscLba);
        let p = f.plan();
        let path = if name == "replacement" {
            f.replacement.clone()
        } else {
            f.descriptor.parent().unwrap().join(name)
        };
        let mut b = fs::read(&path).unwrap();
        b[0] ^= 1;
        fs::write(&path, b).unwrap();
        assert!(p.prepare().is_err(), "{name}");
        assert!(!f.destination.exists());
    }
}
#[test]
fn changed_after_staging_refuses_publication() {
    let f = Fixture::new(SectorFormat::Cooked2048, ExtentCoordinates::DiscLba);
    let p = f.plan().prepare().unwrap();
    fs::write(&f.replacement, vec![0; 3000]).unwrap();
    assert!(tree::publish(&p.journal_path).is_err());
    assert!(!f.destination.exists());
}
#[test]
fn deterministic_faults_never_publish_or_modify_source() {
    for fault in [
        Fault::Staging,
        Fault::SecondTrackCopy,
        Fault::Regeneration,
        Fault::SectorWrite,
        Fault::Verification,
        Fault::Receipt,
    ] {
        let f = Fixture::new(SectorFormat::Mode1Raw2352, ExtentCoordinates::DiscLba);
        let before = contract::snapshot(f.descriptor.parent().unwrap());
        let p = f.plan();
        assert!(p.prepare_with_fault(Some(fault)).is_err(), "{fault:?}");
        assert!(!f.destination.exists());
        assert_eq!(contract::snapshot(f.descriptor.parent().unwrap()), before);
        for e in fs::read_dir(f.temp.path()).unwrap() {
            let path = e.unwrap().path();
            if path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".emuwiz-patch-tree-")
                && path.extension().is_some_and(|s| s == "json")
            {
                assert!(tree::publish(&path).is_err());
            }
        }
    }
}
#[test]
fn publication_collision_retains_verified_stage_and_retry_works() {
    let f = Fixture::new(SectorFormat::Mode1Raw2352, ExtentCoordinates::DiscLba);
    let p = f.plan().prepare().unwrap();
    fs::create_dir(&f.destination).unwrap();
    fs::write(f.destination.join("external"), b"retain").unwrap();
    assert!(tree::publish(&p.journal_path).is_err());
    assert_eq!(fs::read(f.destination.join("external")).unwrap(), b"retain");
    fs::remove_file(f.destination.join("external")).unwrap();
    fs::remove_dir(&f.destination).unwrap();
    assert_eq!(
        tree::inspect(&p.journal_path).unwrap(),
        TreePatchState::Staged
    );
    tree::publish(&p.journal_path).unwrap();
}
#[test]
fn corrupted_changed_or_untouched_sector_cannot_publish() {
    for sector in [21, 24] {
        let f = Fixture::new(SectorFormat::Mode1Raw2352, ExtentCoordinates::DiscLba);
        let plan = f.plan();
        let p = plan.prepare().unwrap();
        let staging = p
            .journal_path
            .with_file_name(p.journal_path.file_stem().unwrap());
        let path = staging.join("track03.bin");
        let mut b = fs::read(&path).unwrap();
        b[sector * 2352 + 16] ^= 1;
        fs::write(&path, b).unwrap();
        assert!(plan.verify(&staging).is_err());
        assert!(tree::publish(&p.journal_path).is_err());
    }
}
#[test]
fn output_changes_refuse_undo() {
    let f = Fixture::new(SectorFormat::Cooked2048, ExtentCoordinates::DiscLba);
    let p = f.plan().prepare().unwrap();
    tree::publish(&p.journal_path).unwrap();
    fs::write(f.destination.join("track02.raw"), b"changed").unwrap();
    assert!(tree::undo(&p.journal_path).is_err());
    assert!(f.destination.exists());
}
#[test]
fn unsupported_layout_modes_offsets_and_missing_tracks_refused() {
    for change in [
        "mode2",
        "edc",
        "offset",
        "short",
        "missing",
        "cdi",
        "coordinates",
    ] {
        let f = Fixture::new(SectorFormat::Mode1Raw2352, ExtentCoordinates::DiscLba);
        let path = f.source.data().source_path.clone();
        match change {
            "mode2" => {
                let mut b = fs::read(&path).unwrap();
                b[15] = 2;
                fs::write(path, b).unwrap();
            }
            "edc" => {
                let mut b = fs::read(&path).unwrap();
                b[24 * 2352 + 2064] ^= 1;
                fs::write(path, b).unwrap();
            }
            "offset" => {
                let b = fs::read_to_string(&f.descriptor)
                    .unwrap()
                    .replace("track03.bin 0", "track03.bin 1");
                fs::write(&f.descriptor, b).unwrap();
            }
            "short" => {
                OpenOptions::new()
                    .write(true)
                    .open(path)
                    .unwrap()
                    .set_len(2352 * 32 - 1)
                    .unwrap();
            }
            "missing" => {
                fs::remove_file(path).unwrap();
            }
            "cdi" => {
                let p = f.temp.path().join("disc.cdi");
                fs::copy(&f.descriptor, &p).unwrap();
                assert!(inspect_gdi_for_extent_patching(&p, ExtentCoordinates::DiscLba).is_err());
                continue;
            }
            _ => {
                assert!(
                    inspect_gdi_for_extent_patching(
                        &f.descriptor,
                        ExtentCoordinates::TrackRelativeLba
                    )
                    .is_err()
                );
                continue;
            }
        }
        assert!(
            inspect_gdi_for_extent_patching(&f.descriptor, ExtentCoordinates::DiscLba).is_err(),
            "{change}"
        );
    }
}
#[test]
fn unsupported_records_overlap_and_ambiguous_names_refused() {
    for change in [
        "multi",
        "interleave",
        "attribute",
        "overlap",
        "name",
        "encoding",
        "directory-cycle",
        "path-table",
    ] {
        let f = Fixture::new(SectorFormat::Cooked2048, ExtentCoordinates::DiscLba);
        let path = f.source.data().source_path.clone();
        let mut b = fs::read(&path).unwrap();
        let r = 20 * 2048 + 68;
        match change {
            "multi" => b[r + 25] = 0x80,
            "interleave" => b[r + 26] = 1,
            "attribute" => b[r + 1] = 1,
            "overlap" => both32(&mut b[r + 2..r + 10], 45020),
            "name" => {
                let next = r + b[r] as usize;
                let rec = record(b"1ST_READ.BIN;2", 45023, 16, false);
                b[next..next + rec.len()].copy_from_slice(&rec);
            }
            "encoding" => b[r + 33] = 0xff,
            "directory-cycle" => {
                b[r + 25] = 2;
                both32(&mut b[r + 2..r + 10], 45020);
                both32(&mut b[r + 10..r + 18], 2048);
            }
            _ => {
                b[16 * 2048 + 140..16 * 2048 + 144].copy_from_slice(&45020u32.to_le_bytes());
            }
        }
        fs::write(path, b).unwrap();
        assert!(
            inspect_gdi_for_extent_patching(&f.descriptor, ExtentCoordinates::DiscLba).is_err(),
            "{change}"
        );
    }
}
#[test]
fn symlink_sources_replacements_and_existing_output_refused() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new(SectorFormat::Cooked2048, ExtentCoordinates::DiscLba);
    let linked = f.temp.path().join("link");
    symlink(&f.replacement, &linked).unwrap();
    let mut r = f.request();
    r.replacement = linked;
    assert!(
        review_gdi_extent_patch(&f.source, f.source.sha256(), &[r], &[], &f.destination).is_err()
    );
    fs::create_dir(&f.destination).unwrap();
    assert!(
        review_gdi_extent_patch(
            &f.source,
            f.source.sha256(),
            &[f.request()],
            &[],
            &f.destination
        )
        .is_err()
    );
}
#[test]
fn shared_immutable_recovery_contract() {
    let fresh = || {
        let f = Fixture::new(SectorFormat::Cooked2048, ExtentCoordinates::DiscLba);
        let p = f.plan().prepare().unwrap();
        let inputs = vec![f.descriptor.parent().unwrap().to_owned(), f.replacement];
        (f.temp, inputs, f.destination, p)
    };
    contract::published_changes_never_gain_undo_authority(&fresh);
}

#[test]
fn injected_publication_failure_retains_recoverable_verified_stage() {
    let f = Fixture::new(SectorFormat::Mode1Raw2352, ExtentCoordinates::DiscLba);
    let before = contract::snapshot(f.descriptor.parent().unwrap());
    let p = f.plan().prepare().unwrap();
    assert!(tree::fail_publication_before_rename(&p.journal_path).is_err());
    assert!(!f.destination.exists());
    assert_eq!(
        tree::inspect(&p.journal_path).unwrap(),
        TreePatchState::Staged
    );
    tree::publish(&p.journal_path).unwrap();
    assert_eq!(
        tree::inspect(&p.journal_path).unwrap(),
        TreePatchState::Published
    );
    assert_eq!(contract::snapshot(f.descriptor.parent().unwrap()), before);
}

#[test]
fn partial_sector_write_failure_is_unpublishable() {
    let f = Fixture::new(SectorFormat::Mode1Raw2352, ExtentCoordinates::DiscLba);
    let plan = f.plan();
    assert!(plan.prepare_with_fault(Some(Fault::SectorWrite)).is_err());
    let stage = fs::read_dir(f.temp.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.is_dir()
                && p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".emuwiz-patch-tree-")
        })
        .unwrap();
    let bytes = fs::read(stage.join("track03.bin")).unwrap();
    assert_eq!(
        &bytes[21 * 2352..22 * 2352],
        plan.sectors[&21].after.as_slice()
    );
    assert_eq!(
        &bytes[22 * 2352..23 * 2352],
        plan.sectors[&22].before.as_slice()
    );
    assert!(!stage.join(RECEIPT_NAME).exists());
    assert!(tree::publish(&stage.with_extension("json")).is_err());
    assert!(!f.destination.exists());
}

#[test]
fn nested_directory_extent_is_mapped_and_replaced() {
    let f = Fixture::new(SectorFormat::Cooked2048, ExtentCoordinates::DiscLba);
    let path = &f.source.data().source_path;
    let mut b = fs::read(path).unwrap();
    let start = 20 * 2048 + 68;
    let r = record(b"DIR", 45024, 2048, true);
    b[start..21 * 2048].fill(0);
    b[start..start + r.len()].copy_from_slice(&r);
    let records = [
        record(&[0], 45024, 2048, true),
        record(&[1], 45020, 2048, true),
        record(b"1ST_READ.BIN;1", 45021, 3000, false),
    ];
    let mut cursor = 24 * 2048;
    for r in records {
        b[cursor..cursor + r.len()].copy_from_slice(&r);
        cursor += r.len();
    }
    // Keep the root boot target separately; move the nested file to sector 25.
    let rootboot = record(b"1ST_READ.BIN;1", 45021, 3000, false);
    b[start + r.len()..start + r.len() + rootboot.len()].copy_from_slice(&rootboot);
    let nested = 24 * 2048 + 68;
    both32(&mut b[nested + 2..nested + 10], 45025);
    b[25 * 2048..25 * 2048 + 3000].fill(0x63);
    fs::write(path, b).unwrap();
    let source =
        inspect_gdi_for_extent_patching(&f.descriptor, ExtentCoordinates::DiscLba).unwrap();
    let request = FileReplacement {
        filesystem_path: "DIR/1ST_READ.BIN".into(),
        expected_source_sha256: source.extents.0["DIR/1ST_READ.BIN"].source_sha256.clone(),
        replacement: f.replacement.clone(),
    };
    let plan =
        review_gdi_extent_patch(&source, source.sha256(), &[request], &[], &f.destination).unwrap();
    let p = plan.prepare().unwrap();
    tree::publish(&p.journal_path).unwrap();
    plan.verify(&f.destination).unwrap();
}

#[test]
fn iso_system_use_extensions_and_unsafe_names_refused() {
    for change in ["system-use", "traversal"] {
        let f = Fixture::new(SectorFormat::Cooked2048, ExtentCoordinates::DiscLba);
        let path = &f.source.data().source_path;
        let mut b = fs::read(path).unwrap();
        let start = 20 * 2048 + 68;
        if change == "system-use" {
            let old = b[start] as usize;
            let next = start + old;
            b.copy_within(next..21 * 2048 - 8, next + 8);
            b[start] = (old + 8) as u8;
            b[start + old..start + old + 8].copy_from_slice(b"SL\x08\x01\x00\x00\x00\x00");
        } else {
            let r = record(b"../ESCAPE;1", 45021, 3000, false);
            b[start..21 * 2048].fill(0);
            b[start..start + r.len()].copy_from_slice(&r);
        }
        fs::write(path, b).unwrap();
        assert!(
            inspect_gdi_for_extent_patching(&f.descriptor, ExtentCoordinates::DiscLba).is_err(),
            "{change}"
        );
    }
}

#[test]
fn ip_bin_region_and_boot_target_policy_is_not_weakened() {
    let f = Fixture::new(SectorFormat::Mode1Raw2352, ExtentCoordinates::DiscLba);
    let edits = [IpBinEdit::Text {
        field: IpBinTextField::BootFilename,
        value: "MISSING.BIN".into(),
    }];
    assert!(
        review_gdi_extent_patch(&f.source, f.source.sha256(), &[], &edits, &f.destination).is_err()
    );
    let edits = [IpBinEdit::Region {
        region: ip_bin::DreamcastRegion::UsaCanada,
        enabled: true,
    }];
    assert!(
        review_gdi_extent_patch(&f.source, f.source.sha256(), &[], &edits, &f.destination).is_err()
    );
}

#[test]
fn crash_state_and_stale_inputs_share_existing_tree_recovery_contract() {
    let fresh = || {
        let f = Fixture::new(SectorFormat::Cooked2048, ExtentCoordinates::DiscLba);
        let p = f.plan().prepare().unwrap();
        let inputs = vec![f.descriptor.parent().unwrap().to_owned(), f.replacement];
        (f.temp, inputs, f.destination, p)
    };
    contract::lifecycle_after_interruption_and_stale_plans_keep_inputs_intact(&fresh, &|inputs| {
        fs::write(inputs[0].join("track02.raw"), b"stale audio").unwrap();
    });
}
