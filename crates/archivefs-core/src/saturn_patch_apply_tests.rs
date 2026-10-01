use super::*;
use crate::patch_output_recovery::tree::{TreePatchState, inspect, publish, undo};
fn var(mut value: u64) -> Vec<u8> {
    let mut encoded = Vec::new();
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            encoded.push(byte | 0x80);
            break;
        }
        encoded.push(byte);
        value -= 1;
    }
    encoded
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn system_id() -> Vec<u8> {
    let mut bytes = vec![b' '; 0x100];
    bytes[..16].copy_from_slice(b"SEGA SEGASATURN ");
    bytes[0x10..0x20].copy_from_slice(b"SEGA ENTERPRISES");
    bytes[0x20..0x2a].copy_from_slice(b"T-1234G   ");
    bytes[0x2a..0x30].copy_from_slice(b"V1.000");
    bytes[0x30..0x38].copy_from_slice(b"19960101");
    bytes[0x38..0x40].copy_from_slice(b"CD-1/1  ");
    bytes[0x40..0x4a].copy_from_slice(b"JTU       ");
    bytes
}

fn patch_bytes(format: &str, source: &[u8], offset: usize, value: u8) -> Vec<u8> {
    let mut target = source.to_vec();
    target[offset] = value;
    match format {
        "ips" => {
            let mut patch = b"PATCH".to_vec();
            patch.extend(((source.len() - 1) as u32).to_be_bytes()[1..].iter());
            patch.extend((1_u16).to_be_bytes());
            patch.push(source[source.len() - 1]);
            patch.extend((offset as u32).to_be_bytes()[1..].iter());
            patch.extend((1_u16).to_be_bytes());
            patch.push(value);
            patch.extend(b"EOF");
            patch
        }
        "bps" => {
            let mut body = Vec::new();
            body.extend(var(((offset as u64 - 1) << 2) | 0));
            body.extend(var((1_u64 - 1) << 2 | 1));
            body.push(value);
            body.extend(var((((source.len() - offset - 1) as u64 - 1) << 2) | 0));
            let mut patch = b"BPS1".to_vec();
            patch.extend(var(source.len() as u64));
            patch.extend(var(target.len() as u64));
            patch.push(0x80);
            patch.extend(body);
            patch.extend(crc32(source).to_le_bytes());
            patch.extend(crc32(&target).to_le_bytes());
            let patch_crc = crc32(&patch);
            patch.extend(patch_crc.to_le_bytes());
            patch
        }
        "ups" => {
            let mut patch = b"UPS1".to_vec();
            patch.extend(var(source.len() as u64));
            patch.extend(var(target.len() as u64));
            patch.extend(var(offset as u64));
            patch.push(source[offset] ^ value);
            patch.push(0);
            patch.extend(crc32(source).to_le_bytes());
            patch.extend(crc32(&target).to_le_bytes());
            let patch_crc = crc32(&patch);
            patch.extend(patch_crc.to_le_bytes());
            patch
        }
        "ppf" => {
            let mut patch = vec![0_u8; 61];
            patch[..5].copy_from_slice(b"PPF30");
            patch[55..59].copy_from_slice(&(source.len() as u32).to_le_bytes());
            patch.extend((offset as u64).to_le_bytes());
            patch.push(1);
            patch.push(value);
            patch
        }
        _ => panic!("unknown fixture format"),
    }
}

pub(crate) fn fixture() -> (tempfile::TempDir, SaturnPatchPlan, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("empty")).unwrap();
    let cue = root.join("game.cue");
    fs::write(&cue, concat!("REM COMMENT \"kept annotation\"\nTITLE \"Reviewed disc\"\n", "FILE \"data.bin\" BINARY\n TRACK 01 MODE1/2048\n INDEX 00 00:00:00\n INDEX 01 00:00:10\n PREGAP 00:00:10\n", "FILE \"audio.bin\" BINARY\n TRACK 02 AUDIO\n INDEX 01 00:00:00\n POSTGAP 00:00:02\n")).unwrap();
    let mut data = vec![0x11; 2048 * 20];
    data[2048 * 10..2048 * 10 + 256].copy_from_slice(&system_id());
    fs::write(root.join("data.bin"), &data).unwrap();
    fs::write(root.join("audio.bin"), vec![0x55; 2352 * 2]).unwrap();
    fs::write(root.join("notes.txt"), b"unchanged").unwrap();
    let patch = temp.path().join("patch.ips");
    fs::write(&patch, patch_bytes("ips", &data, 30000, 0x33)).unwrap();
    let manifest = inspect_saturn_disc(&cue).unwrap();
    let binding = binding(&manifest, &patch);
    let destination = temp.path().join("published");
    let plan = review_saturn_patch(&patch, &destination, &binding).unwrap();
    (temp, plan, destination)
}
fn binding(manifest: &SaturnDiscManifest, patch: &Path) -> SaturnPatchBinding {
    let component = manifest
        .components
        .iter()
        .find(|c| c.path.ends_with("data.bin"))
        .unwrap();
    SaturnPatchBinding {
        manifest: manifest.clone(),
        target: SaturnPatchTarget::ComponentBin {
            component: component.path.clone(),
            sha256: component.sha256.clone(),
        },
        patch_sha256: file_content(patch, MAX_PATCH_BYTES).unwrap().sha256,
        track_number: 1,
        disc_ordinal: 1,
        patch_disc_ordinal: 1,
    }
}
#[test]
fn all_supported_formats_preserve_audio_layout_and_sources() {
    for format in ["ips", "bps", "ups", "ppf"] {
        let (_temp, initial, destination) = fixture();
        let source = fs::read(initial.source.join("data.bin")).unwrap();
        fs::write(&initial.patch, patch_bytes(format, &source, 30000, 0x33)).unwrap();
        let plan = review_saturn_patch(
            &initial.patch,
            &destination,
            &binding(&initial.manifest, &initial.patch),
        )
        .unwrap();
        let patch_before = fs::read(&plan.patch).unwrap();
        let modified = fs::metadata(plan.source.join("data.bin"))
            .unwrap()
            .modified()
            .unwrap();
        let prepared = plan.prepare().unwrap();
        assert_eq!(
            inspect(&prepared.journal_path).unwrap(),
            TreePatchState::Staged
        );
        publish(&prepared.journal_path).unwrap();
        assert_eq!(
            inspect(&prepared.journal_path).unwrap(),
            TreePatchState::Published
        );
        assert_eq!(fs::read(destination.join("data.bin")).unwrap()[30000], 0x33);
        assert_eq!(
            fs::read(destination.join("audio.bin")).unwrap(),
            fs::read(plan.source.join("audio.bin")).unwrap()
        );
        assert_eq!(
            fs::read(destination.join("game.cue")).unwrap(),
            fs::read(plan.source.join("game.cue")).unwrap()
        );
        plan.original
            .verify(&plan.source, MAX_SOURCE_BYTES)
            .unwrap();
        assert_eq!(fs::read(&plan.patch).unwrap(), patch_before);
        assert_eq!(
            fs::metadata(plan.source.join("data.bin"))
                .unwrap()
                .modified()
                .unwrap(),
            modified
        );
    }
}
#[test]
fn shared_recovery_collision_undo_republish_and_corruption() {
    let (_temp, plan, destination) = fixture();
    let prepared = plan.prepare().unwrap(); // stop/crash before publish, durable receipt
    assert_eq!(
        inspect(&prepared.journal_path).unwrap(),
        TreePatchState::Staged
    );
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("owner"), b"keep").unwrap();
    assert!(publish(&prepared.journal_path).is_err());
    assert_eq!(fs::read(destination.join("owner")).unwrap(), b"keep");
    fs::remove_file(destination.join("owner")).unwrap();
    fs::remove_dir(&destination).unwrap();
    publish(&prepared.journal_path).unwrap(); // stop/crash after publish: no private completion record
    assert_eq!(
        inspect(&prepared.journal_path).unwrap(),
        TreePatchState::Published
    );
    undo(&prepared.journal_path).unwrap();
    assert_eq!(
        inspect(&prepared.journal_path).unwrap(),
        TreePatchState::Staged
    );
    assert!(undo(&prepared.journal_path).is_err());
    publish(&prepared.journal_path).unwrap();
    fs::write(destination.join("audio.bin"), b"changed").unwrap();
    assert!(undo(&prepared.journal_path).is_err());
    assert!(destination.exists());
}
#[test]
fn stale_dependencies_refuse_before_patcher() {
    for change in [
        "patch",
        "target",
        "added",
        "removed",
        "renamed",
        "cue",
        "destination",
    ] {
        let (_temp, plan, destination) = fixture();
        match change {
            "patch" => fs::write(&plan.patch, b"changed").unwrap(),
            "target" => fs::write(plan.source.join("data.bin"), b"changed").unwrap(),
            "added" => fs::write(plan.source.join("new"), b"new").unwrap(),
            "removed" => fs::remove_file(plan.source.join("notes.txt")).unwrap(),
            "renamed" => {
                fs::rename(plan.source.join("notes.txt"), plan.source.join("renamed")).unwrap()
            }
            "cue" => fs::write(plan.source.join("game.cue"), b"changed").unwrap(),
            _ => fs::create_dir(&destination).unwrap(),
        }
        let called = std::cell::Cell::new(false);
        assert!(
            tree::prepare(
                &plan.tree,
                |_| {
                    called.set(true);
                    Ok(())
                },
                |_| Ok(())
            )
            .is_err(),
            "{change}"
        );
        assert!(!called.get(), "{change}");
        assert!(plan.prepare().is_err(), "{change}");
    }
}
#[test]
fn wrong_component_track_ordinal_and_manifest_refused() {
    let (_temp, plan, destination) = fixture();
    for change in ["identity", "track", "audio", "logical", "ordinal", "cue"] {
        let mut b = binding(&plan.manifest, &plan.patch);
        match change {
            "identity" => {
                if let SaturnPatchTarget::ComponentBin { sha256, .. } = &mut b.target {
                    *sha256 = "0".repeat(64);
                }
            }
            "track" => b.track_number = 2,
            "audio" => {
                let c = b
                    .manifest
                    .components
                    .iter()
                    .find(|c| c.path.ends_with("audio.bin"))
                    .unwrap();
                b.target = SaturnPatchTarget::ComponentBin {
                    component: c.path.clone(),
                    sha256: c.sha256.clone(),
                };
                b.track_number = 2;
            }
            "logical" => {
                b.target = SaturnPatchTarget::LogicalDataTrack {
                    track_number: 1,
                    sha256: b.manifest.tracks[0].data_logical_sha256.clone().unwrap(),
                }
            }
            "ordinal" => b.patch_disc_ordinal = 2,
            _ => b.manifest.descriptor_sha256 = "0".repeat(64),
        }
        assert!(
            review_saturn_patch(&plan.patch, &destination, &b).is_err(),
            "{change}"
        );
    }
}
#[test]
fn explicit_ips_truncation_remains_canonical_and_cannot_publish() {
    let (_temp, plan, destination) = fixture();
    let mut patch = fs::read(&plan.patch).unwrap();
    patch.extend([0, 0, 1]);
    fs::write(&plan.patch, patch).unwrap();
    let inspection = inspect_standalone_patch(&plan.patch).unwrap();
    let engine = build_standalone_patch_apply_plan(
        &inspection,
        plan.source.join("data.bin"),
        &destination,
        destination.parent().unwrap(),
    )
    .unwrap();
    assert_eq!(
        prepare_standalone_patch_output(&engine)
            .unwrap()
            .bytes
            .len(),
        1
    );
    let review = review_saturn_patch(
        &plan.patch,
        &destination,
        &binding(&plan.manifest, &plan.patch),
    )
    .unwrap();
    assert!(review.prepare().is_err());
    assert!(!destination.exists());
}
#[test]
fn unsupported_ssp_chd_xdelta_malformed_and_image_refused() {
    let (temp, plan, destination) = fixture();
    for (name, bytes) in [
        ("opaque.ssp", b"PATCH".as_slice()),
        ("patch.xdelta", b"\xd6\xc3\xc4\0".as_slice()),
        ("unknown.patch", b"unknown".as_slice()),
        ("bad.ips", b"PATCH\0".as_slice()),
    ] {
        let path = temp.path().join(name);
        fs::write(&path, bytes).unwrap();
        assert!(
            review_saturn_patch(&path, &destination, &binding(&plan.manifest, &path)).is_err(),
            "{name}"
        );
    }
    let mut b = binding(&plan.manifest, &plan.patch);
    b.manifest.source_descriptor = temp.path().join("disc.chd");
    assert!(review_saturn_patch(&plan.patch, &destination, &b).is_err());
}
#[test]
fn patched_native_identity_or_layout_corruption_refused() {
    for (offset, value) in [
        (20480, b'X'),
        (20480 + 0x20, b'X'),
        (20480 + 0x2a, b'X'),
        (20480 + 0x40, b'E'),
    ] {
        let (_temp, plan, destination) = fixture();
        let data = fs::read(plan.source.join("data.bin")).unwrap();
        fs::write(&plan.patch, patch_bytes("ips", &data, offset, value)).unwrap();
        let review = review_saturn_patch(
            &plan.patch,
            &destination,
            &binding(&plan.manifest, &plan.patch),
        )
        .unwrap();
        assert!(review.prepare().is_err());
        assert!(!destination.exists());
    }
    for member in ["audio.bin", "game.cue", "unexpected"] {
        let (_temp, plan, destination) = fixture();
        let content = RefCell::new(None);
        let result = tree::prepare(
            &plan.tree,
            |stage| {
                *content.borrow_mut() = Some(plan.produce(stage)?);
                fs::write(stage.join(member), b"changed")?;
                Ok(())
            },
            |stage| plan.verify(stage, content.borrow().as_ref().unwrap()),
        );
        assert!(result.is_err());
        assert!(!destination.exists());
    }
}
#[test]
fn native_ordinal_and_logical_size_policy_refused() {
    let (_temp, plan, destination) = fixture();
    assert!(plan.max_total_bytes() <= MAX_SOURCE_BYTES + MAX_PATCH_BYTES);
    let mut b = binding(&plan.manifest, &plan.patch);
    b.disc_ordinal = 2;
    b.patch_disc_ordinal = 2;
    assert!(review_saturn_patch(&plan.patch, &destination, &b).is_err());
    let b = binding(&plan.manifest, &plan.patch);
    fs::File::create(&plan.patch)
        .unwrap()
        .set_len(MAX_PATCH_BYTES + 1)
        .unwrap();
    assert!(review_saturn_patch(&plan.patch, &destination, &b).is_err());
    fs::File::create(plan.source.join("sparse"))
        .unwrap()
        .set_len(MAX_SOURCE_BYTES + 1)
        .unwrap();
    assert!(Contents::read(&plan.source, MAX_SOURCE_BYTES).is_err());
}

fn raw_fixture(mode2: bool) -> (tempfile::TempDir, SaturnPatchPlan, PathBuf) {
    use crate::raw_cd_sector::{RAW_SECTOR_BYTES, SYNC_PATTERN};
    let (temp, cooked, destination) = fixture();
    let source = fs::read(cooked.source.join("data.bin")).unwrap();
    let header = if mode2 { 24 } else { 16 };
    let mut raw = Vec::new();
    for sector in source.chunks_exact(2048) {
        let mut bytes = vec![0; RAW_SECTOR_BYTES];
        bytes[..12].copy_from_slice(&SYNC_PATTERN);
        bytes[15] = if mode2 { 2 } else { 1 };
        bytes[header..header + 2048].copy_from_slice(sector);
        raw.extend(bytes);
    }
    fs::write(cooked.source.join("data.bin"), &raw).unwrap();
    let cue = cooked.source.join("game.cue");
    let text = fs::read_to_string(&cue).unwrap().replace(
        "MODE1/2048",
        if mode2 { "MODE2/2352" } else { "MODE1/2352" },
    );
    fs::write(&cue, text).unwrap();
    let manifest = inspect_saturn_disc(&cue).unwrap();
    fs::write(
        &cooked.patch,
        patch_bytes("ips", &raw, 11 * 2352 + header + 300, 0x33),
    )
    .unwrap();
    let plan = review_saturn_patch(
        &cooked.patch,
        &destination,
        &binding(&manifest, &cooked.patch),
    )
    .unwrap();
    (temp, plan, destination)
}
#[test]
fn raw_component_payload_patches_preserve_sector_headers() {
    for mode2 in [false, true] {
        let (_temp, plan, destination) = raw_fixture(mode2);
        let prepared = plan.prepare().unwrap();
        publish(&prepared.journal_path).unwrap();
        plan.verify_raw_headers(&destination).unwrap();
        let header = if mode2 { 24 } else { 16 };
        assert_eq!(
            fs::read(destination.join("data.bin")).unwrap()[11 * 2352 + header + 300],
            0x33
        );
        plan.original
            .verify(&plan.source, MAX_SOURCE_BYTES)
            .unwrap();
    }
}
#[test]
fn raw_sector_mode_address_and_xa_subheader_changes_refused() {
    for (mode2, offset, value) in [(false, 12, 1), (false, 15, 2), (true, 16, 1)] {
        let (_temp, plan, destination) = raw_fixture(mode2);
        let raw = fs::read(plan.source.join("data.bin")).unwrap();
        fs::write(
            &plan.patch,
            patch_bytes("ips", &raw, 11 * 2352 + offset, value),
        )
        .unwrap();
        let review = review_saturn_patch(
            &plan.patch,
            &destination,
            &binding(&plan.manifest, &plan.patch),
        )
        .unwrap();
        assert!(review.prepare().is_err());
        assert!(!destination.exists());
    }
    let (_temp, plan, destination) = raw_fixture(true);
    let cue = plan.source.join("game.cue");
    fs::write(
        &cue,
        fs::read_to_string(&cue)
            .unwrap()
            .replace("MODE2/2352", "MODE1/2352"),
    )
    .unwrap();
    let manifest = inspect_saturn_disc(&cue).unwrap();
    assert!(
        review_saturn_patch(&plan.patch, &destination, &binding(&manifest, &plan.patch)).is_err()
    );
}
#[test]
fn cue_admission_refuses_dropped_semantics_and_unicode_directives_without_panic() {
    let (_temp, plan, destination) = fixture();
    let cue = plan.source.join("game.cue");
    let original = fs::read_to_string(&cue).unwrap();
    for line in [
        "INDEX 02 00:00:12",
        "FLAGS PRE",
        "REM SESSION 2",
        "未知结构",
        "FILE \"data.bin\" MOTOROLA",
    ] {
        fs::write(&cue, format!("{original}\n{line}\n")).unwrap();
        // Supply fresh matching evidence for directives the inspection parser
        // silently drops, so only admission (not a stale hash) refuses them.
        let manifest = if line.is_ascii() {
            inspect_saturn_disc(&cue).unwrap()
        } else {
            plan.manifest.clone()
        };
        assert!(
            review_saturn_patch(&plan.patch, &destination, &binding(&manifest, &plan.patch))
                .is_err(),
            "{line}"
        );
    }
    fs::write(&cue, original.replace("Reviewed disc", "遊戯ディスク")).unwrap();
    let manifest = inspect_saturn_disc(&cue).unwrap();
    let reviewed =
        review_saturn_patch(&plan.patch, &destination, &binding(&manifest, &plan.patch)).unwrap();
    reviewed.prepare().unwrap();
}

fn fresh_published() -> (tempfile::TempDir, Vec<PathBuf>, PathBuf, PreparedTreePatch) {
    let (temp, plan, destination) = fixture();
    let prepared = plan.prepare().unwrap();
    (
        temp,
        vec![plan.source.clone(), plan.patch.clone()],
        destination,
        prepared,
    )
}
#[test]
fn shared_contract_published_changes_never_gain_undo_authority() {
    crate::optical_patch_tree::contract::published_changes_never_gain_undo_authority(
        &fresh_published,
    );
}
#[test]
fn shared_contract_interruption_recovery_and_input_immutability() {
    crate::optical_patch_tree::contract::lifecycle_after_interruption_and_stale_plans_keep_inputs_intact(
        &fresh_published,
        &|inputs| {
            // Change the reviewed patch after review.
            let mut bytes = fs::read(&inputs[1]).unwrap();
            let last = bytes.len() - 1;
            bytes[last] ^= 1;
            fs::write(&inputs[1], bytes).unwrap();
        },
    );
    // Changing an untouched audio component is stale too.
    let (_temp, plan, destination) = fixture();
    let prepared = plan.prepare().unwrap();
    let audio = fs::read_dir(&plan.source)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension().is_some_and(|e| e == "bin") && *path != plan.source.join(&plan.target)
        })
        .expect("fixture has an audio component");
    fs::write(&audio, b"changed audio").unwrap();
    assert!(publish(&prepared.journal_path).is_err());
    assert!(!destination.exists());
}
#[test]
fn ips_record_past_eof_grows_in_the_engine_but_never_publishes() {
    let (_temp, plan, destination) = fixture();
    let before = crate::optical_patch_tree::contract::snapshot(&plan.source);
    let size = fs::metadata(plan.source.join("data.bin")).unwrap().len();
    // One IPS record exactly at EOF: the canonical engine grows the output.
    let mut patch = b"PATCH".to_vec();
    patch.extend(&(size as u32).to_be_bytes()[1..]);
    patch.extend([0, 1, 0x33]);
    patch.extend(b"EOF");
    fs::write(&plan.patch, &patch).unwrap();
    let inspection = inspect_standalone_patch(&plan.patch).unwrap();
    let engine = build_standalone_patch_apply_plan(
        &inspection,
        plan.source.join("data.bin"),
        &destination,
        destination.parent().unwrap(),
    )
    .unwrap();
    assert_eq!(
        prepare_standalone_patch_output(&engine)
            .unwrap()
            .bytes
            .len() as u64,
        size + 1
    );
    // The fixed-layout backend must refuse that growth (the optical layout
    // would silently change), at review or at preparation.
    let result = review_saturn_patch(
        &plan.patch,
        &destination,
        &binding(&plan.manifest, &plan.patch),
    )
    .and_then(|review| review.prepare());
    assert!(result.is_err());
    assert!(!destination.exists());
    assert_eq!(
        crate::optical_patch_tree::contract::snapshot(&plan.source),
        before
    );
}
