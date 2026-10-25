use super::*;
use crate::patch_output_recovery::tree::PreparedTreePatch;
use crate::patch_output_recovery::tree::{TreePatchState, inspect, publish, undo};
use std::io::Write;

pub(crate) fn ip_bin() -> Vec<u8> {
    let mut b = vec![b' '; 256];
    b[..16].copy_from_slice(b"SEGA SEGAKATANA ");
    b[16..32].copy_from_slice(b"SEGA ENTERPRISES");
    b[32..48].copy_from_slice(b" GD-ROM         ");
    b[48] = b'J';
    b[64..71].copy_from_slice(b"T-1234M");
    b[74..80].copy_from_slice(b"V1.000");
    b[80..88].copy_from_slice(b"20000101");
    b[96..108].copy_from_slice(b"1ST_READ.BIN");
    b
}
pub(crate) fn package(path: &Path, members: &[(&str, &[u8])]) {
    let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
    for (name, bytes) in members {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
}
pub(crate) fn fixture() -> (tempfile::TempDir, DreamcastDcpPlan, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir_all(source.join("bootsector")).unwrap();
    fs::create_dir(source.join("empty")).unwrap();
    fs::write(source.join("bootsector/IP.BIN"), ip_bin()).unwrap();
    fs::write(source.join("1ST_READ.BIN"), b"old boot").unwrap();
    fs::write(source.join("untouched"), b"retain").unwrap();
    let patch = temp.path().join("patch.dcp");
    package(&patch, &[("1ST_READ.BIN", b"new boot")]);
    let binding = binding(&source, &patch);
    let destination = temp.path().join("published");
    let plan = review_dreamcast_dcp(&source, &patch, &destination, &binding).unwrap();
    (temp, plan, destination)
}
pub(crate) fn binding(source: &Path, patch: &Path) -> DreamcastDcpBinding {
    DreamcastDcpBinding {
        source_tree_sha256: source_tree_sha256(source).unwrap(),
        package_sha256: inspect_dreamcast_dcp(patch).unwrap().package_sha256,
        identity: DreamcastIdentity {
            product_code: "T-1234M".into(),
            revision: "V1.000".into(),
            region: "J".into(),
        },
    }
}
#[test]
fn apply_recovery_undo_and_source_immutability() {
    let (_temp, plan, destination) = fixture();
    let source_before = Contents::read(&plan.source, MAX_SOURCE_BYTES).unwrap();
    let metadata = fs::metadata(plan.source.join("1ST_READ.BIN"))
        .unwrap()
        .modified()
        .unwrap();
    let patch_before = fs::read(&plan.package.path).unwrap();
    let prepared = plan.prepare().unwrap();
    assert_eq!(
        inspect(&prepared.journal_path).unwrap(),
        TreePatchState::Staged
    );
    assert!(!destination.exists()); // simulated stop/crash before publication
    publish(&prepared.journal_path).unwrap();
    assert_eq!(
        inspect(&prepared.journal_path).unwrap(),
        TreePatchState::Published
    );
    assert_eq!(
        fs::read(destination.join("1ST_READ.BIN")).unwrap(),
        b"new boot"
    );
    undo(&prepared.journal_path).unwrap();
    assert!(!destination.exists());
    assert_eq!(
        inspect(&prepared.journal_path).unwrap(),
        TreePatchState::Staged
    );
    assert!(undo(&prepared.journal_path).is_err());
    publish(&prepared.journal_path).unwrap();
    fs::write(destination.join("unexpected"), b"external").unwrap();
    assert!(undo(&prepared.journal_path).is_err());
    source_before
        .verify(&plan.source, MAX_SOURCE_BYTES)
        .unwrap();
    assert_eq!(
        fs::metadata(plan.source.join("1ST_READ.BIN"))
            .unwrap()
            .modified()
            .unwrap(),
        metadata
    );
    assert_eq!(fs::read(&plan.package.path).unwrap(), patch_before);
}
#[test]
fn stale_dependencies_refuse_before_producer() {
    for change in [
        "patch",
        "target",
        "added",
        "removed",
        "renamed",
        "ip",
        "destination",
    ] {
        let (_temp, plan, destination) = fixture();
        match change {
            "patch" => fs::write(&plan.package.path, b"changed").unwrap(),
            "target" => fs::write(plan.source.join("1ST_READ.BIN"), b"changed").unwrap(),
            "added" => fs::write(plan.source.join("new"), b"new").unwrap(),
            "removed" => fs::remove_file(plan.source.join("untouched")).unwrap(),
            "renamed" => {
                fs::rename(plan.source.join("untouched"), plan.source.join("renamed")).unwrap()
            }
            "ip" => fs::write(plan.source.join("bootsector/IP.BIN"), b"changed").unwrap(),
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
fn wrong_binding_and_domain_refused() {
    let (_temp, plan, destination) = fixture();
    for field in ["source", "patch", "product", "revision", "region"] {
        let mut b = binding(&plan.source, &plan.package.path);
        match field {
            "source" => b.source_tree_sha256 = "0".repeat(64),
            "patch" => b.package_sha256 = "0".repeat(64),
            "product" => b.identity.product_code = "wrong".into(),
            "revision" => b.identity.revision = "wrong".into(),
            _ => b.identity.region = "wrong".into(),
        }
        assert!(review_dreamcast_dcp(&plan.source, &plan.package.path, &destination, &b).is_err());
    }
    let mut ip = ip_bin();
    ip[0] = b'X';
    fs::write(plan.source.join("bootsector/IP.BIN"), ip).unwrap();
    assert!(
        review_dreamcast_dcp(
            &plan.source,
            &plan.package.path,
            &destination,
            &binding(&plan.source, &plan.package.path)
        )
        .is_err()
    );
}
#[test]
fn unsafe_packages_missing_targets_and_deltas_refused() {
    for name in [
        "../escape",
        "/absolute",
        "C:/absolute",
        "dir\\escape",
        "./alias",
        "a//b",
        "missing",
        "1ST_READ.BIN.xdelta",
    ] {
        let (_temp, plan, destination) = fixture();
        package(&plan.package.path, &[(name, b"data")]);
        if matches!(name, "missing" | "1ST_READ.BIN.xdelta") {
            assert!(
                review_dreamcast_dcp(
                    &plan.source,
                    &plan.package.path,
                    &destination,
                    &binding(&plan.source, &plan.package.path)
                )
                .is_err()
            );
        } else {
            assert!(inspect_dreamcast_dcp(&plan.package.path).is_err(), "{name}");
        }
    }
    for members in [
        vec![
            ("1ST_READ.BIN", b"a".as_slice()),
            ("1st_read.bin", b"b".as_slice()),
        ],
        vec![("a", b"a".as_slice()), ("a/b", b"b".as_slice())],
    ] {
        let (_temp, plan, _) = fixture();
        package(&plan.package.path, &members);
        assert!(inspect_dreamcast_dcp(&plan.package.path).is_err());
    }
}
#[test]
fn ip_replacement_verified_before_publication() {
    let (_temp, plan, destination) = fixture();
    let mut ip = ip_bin();
    ip[128] = b'X';
    package(&plan.package.path, &[("bootsector/IP.BIN", &ip)]);
    let new = review_dreamcast_dcp(
        &plan.source,
        &plan.package.path,
        &destination,
        &binding(&plan.source, &plan.package.path),
    )
    .unwrap();
    let prepared = new.prepare().unwrap();
    publish(&prepared.journal_path).unwrap();
    assert_eq!(fs::read(destination.join("bootsector/IP.BIN")).unwrap(), ip);
}
#[test]
fn failed_partial_apply_verification_and_unexpected_member_never_publish() {
    for failure in ["partial", "extra", "ip", "unchanged"] {
        let (_temp, plan, destination) = fixture();
        let result = tree::prepare(
            &plan.tree,
            |staging| {
                if failure == "partial" {
                    fs::write(staging.join("partial"), b"retained")?;
                    return Err(refuse("injected partial apply failure"));
                }
                plan.produce(staging)?;
                match failure {
                    "extra" => fs::write(staging.join("unexpected"), b"extra")?,
                    "ip" => fs::write(staging.join("bootsector/IP.BIN"), b"invalid")?,
                    _ => fs::write(staging.join("untouched"), b"changed")?,
                }
                Ok(())
            },
            |staging| plan.verify(staging),
        );
        assert!(result.is_err());
        assert!(!destination.exists());
        plan.original
            .verify(&plan.source, MAX_SOURCE_BYTES)
            .unwrap();
    }
    let (_temp, plan, destination) = fixture();
    let mut bad_ip = ip_bin();
    bad_ip[0] = b'X';
    package(&plan.package.path, &[("bootsector/IP.BIN", &bad_ip)]);
    let invalid = review_dreamcast_dcp(
        &plan.source,
        &plan.package.path,
        &destination,
        &binding(&plan.source, &plan.package.path),
    )
    .unwrap();
    assert!(invalid.prepare().is_err()); // correct package content, wrong Dreamcast domain
    assert!(!destination.exists());
}
#[test]
fn direct_gdi_chd_cdi_and_symlink_sources_refused() {
    let (temp, plan, destination) = fixture();
    let b = binding(&plan.source, &plan.package.path);
    for name in ["disc.gdi", "disc.chd", "disc.cdi"] {
        let path = temp.path().join(name);
        fs::write(&path, b"image").unwrap();
        assert!(review_dreamcast_dcp(&path, &plan.package.path, &destination, &b).is_err());
    }
    std::os::unix::fs::symlink(&plan.package.path, plan.source.join("escape")).unwrap();
    assert!(source_tree_sha256(&plan.source).is_err());
}
#[test]
fn publication_collision_never_clobbers() {
    let (_temp, plan, destination) = fixture();
    let prepared = plan.prepare().unwrap();
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("owner"), b"keep").unwrap();
    assert!(publish(&prepared.journal_path).is_err());
    assert_eq!(fs::read(destination.join("owner")).unwrap(), b"keep");
}
#[test]
fn zip_symlinks_special_entries_duplicates_and_malformed_data_refused() {
    let (_temp, plan, _) = fixture();
    {
        let mut zip = zip::ZipWriter::new(File::create(&plan.package.path).unwrap());
        zip.add_symlink(
            "escape",
            "../outside",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.finish().unwrap();
    }
    assert!(inspect_dreamcast_dcp(&plan.package.path).is_err());
    for mode in [0o010644u32, 0o020644, 0o060644, 0o140644] {
        package(&plan.package.path, &[("1ST_READ.BIN", b"new boot")]);
        let mut bytes = fs::read(&plan.package.path).unwrap();
        let central = bytes.windows(4).position(|b| b == b"PK\x01\x02").unwrap();
        bytes[central + 5] = 3; // Unix creator
        bytes[central + 38..central + 42].copy_from_slice(&(mode << 16).to_le_bytes());
        fs::write(&plan.package.path, bytes).unwrap();
        assert!(inspect_dreamcast_dcp(&plan.package.path).is_err());
    }
    package(
        &plan.package.path,
        &[("1ST_READ.BIN", b"a"), ("1st_read.bin", b"b")],
    );
    let mut bytes = fs::read(&plan.package.path).unwrap();
    let positions: Vec<_> = bytes
        .windows(12)
        .enumerate()
        .filter_map(|(i, b)| (b == b"1st_read.bin").then_some(i))
        .collect();
    for at in positions {
        bytes[at..at + 12].copy_from_slice(b"1ST_READ.BIN");
    }
    fs::write(&plan.package.path, bytes).unwrap();
    assert!(inspect_dreamcast_dcp(&plan.package.path).is_err());
    fs::write(&plan.package.path, b"malformed DCP records").unwrap();
    assert!(inspect_dreamcast_dcp(&plan.package.path).is_err());
}
#[test]
fn logical_size_and_package_expansion_bounds_and_real_sha256() {
    let (_temp, plan, _) = fixture();
    assert_eq!(plan.package.package_sha256.len(), 64);
    assert_eq!(
        plan.package.package_sha256,
        crate::optical_patch_tree::digest(&fs::read(&plan.package.path).unwrap())
    );
    assert!(plan.max_total_bytes() < MAX_STAGING_BYTES);
    let mut bytes = fs::read(&plan.package.path).unwrap();
    let central = bytes.windows(4).position(|b| b == b"PK\x01\x02").unwrap();
    bytes[central + 24..central + 28].copy_from_slice(
        &((crate::dreamcast_patch_readiness::MAX_DCP_ENTRY_BYTES + 1) as u32).to_le_bytes(),
    );
    fs::write(&plan.package.path, bytes).unwrap();
    assert!(inspect_dreamcast_dcp(&plan.package.path).is_err());
    File::create(&plan.package.path)
        .unwrap()
        .set_len(MAX_DCP_EXPANDED_BYTES + 1)
        .unwrap();
    assert!(inspect_dreamcast_dcp(&plan.package.path).is_err());
    File::create(plan.source.join("sparse"))
        .unwrap()
        .set_len(MAX_SOURCE_BYTES + 1)
        .unwrap();
    assert!(source_tree_sha256(&plan.source).is_err());
}
#[test]
fn ip_bin_length_change_refused() {
    let (_temp, plan, destination) = fixture();
    let mut ip = ip_bin();
    ip.extend([0; 128]);
    package(&plan.package.path, &[("bootsector/IP.BIN", &ip)]);
    assert!(
        review_dreamcast_dcp(
            &plan.source,
            &plan.package.path,
            &destination,
            &binding(&plan.source, &plan.package.path)
        )
        .is_err()
    );
}

#[test]
fn extracted_tree_larger_than_512_mib_is_planned_under_the_explicit_policy() {
    use crate::patch_output_recovery::tree::HARD_MAX_TOTAL_BYTES;
    const MIB: u64 = 1024 * 1024;
    // The bound is explicit, checked at compile time against the shared helper
    // ceiling, and large enough for a full GD-ROM high-density area.
    const _: () = assert!(MAX_STAGING_BYTES <= HARD_MAX_TOTAL_BYTES);
    const _: () = assert!(MAX_SOURCE_BYTES >= 504_150 * 2048);
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir_all(source.join("bootsector")).unwrap();
    fs::write(source.join("bootsector/IP.BIN"), ip_bin()).unwrap();
    fs::write(source.join("1ST_READ.BIN"), b"old boot").unwrap();
    // Just past the former 512 MiB bound, as a sparse file: logical size counts
    // and nothing proportional to it is held in memory (a 900 MiB tree was
    // measured at 46 MB peak RSS through review, prepare, publish and undo).
    File::create(source.join("bulk.bin"))
        .unwrap()
        .set_len(513 * MIB)
        .unwrap();
    let patch = temp.path().join("patch.dcp");
    package(&patch, &[("1ST_READ.BIN", b"new boot")]);
    let plan = review_dreamcast_dcp(
        &source,
        &patch,
        &temp.path().join("published"),
        &binding(&source, &patch),
    )
    .unwrap();
    assert!(plan.max_total_bytes() > 512 * MIB);
    assert!(plan.max_total_bytes() <= MAX_STAGING_BYTES);
}

fn fresh_published() -> (tempfile::TempDir, Vec<PathBuf>, PathBuf, PreparedTreePatch) {
    let (temp, plan, destination) = fixture();
    let prepared = plan.prepare().unwrap();
    (
        temp,
        vec![plan.source.clone(), plan.package.path.clone()],
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
        &|inputs| fs::write(inputs[0].join("untouched"), b"edited after review").unwrap(),
    );
    // A changed package is just as stale as a changed source tree.
    let (_temp, inputs, destination, prepared) = fresh_published();
    fs::write(&inputs[1], b"not the reviewed package").unwrap();
    assert!(publish(&prepared.journal_path).is_err());
    assert!(!destination.exists());
}
#[test]
fn patched_ip_bin_that_changes_boot_mapping_or_identity_never_publishes() {
    for variant in ["boot member", "product", "revision", "region"] {
        let (_temp, plan, destination) = fixture();
        let mut ip = ip_bin();
        match variant {
            // Same length, valid IP.BIN, but boots a different existing file.
            "boot member" => ip[96..108].copy_from_slice(b"untouched   "),
            "product" => ip[64..71].copy_from_slice(b"T-9999M"),
            "revision" => ip[74..80].copy_from_slice(b"V9.999"),
            _ => ip[48] = b'U',
        }
        package(&plan.package.path, &[("bootsector/IP.BIN", &ip)]);
        let result = review_dreamcast_dcp(
            &plan.source,
            &plan.package.path,
            &destination,
            &binding(&plan.source, &plan.package.path),
        )
        .and_then(|review| review.prepare());
        assert!(result.is_err(), "{variant}");
        assert!(!destination.exists(), "{variant}");
        assert_eq!(
            fs::read(plan.source.join("bootsector/IP.BIN")).unwrap(),
            ip_bin(),
            "{variant}"
        );
    }
}
