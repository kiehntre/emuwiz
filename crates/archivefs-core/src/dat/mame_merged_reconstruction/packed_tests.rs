//! Packed ZIP-member staging: the acceptance matrix. Every case builds a
//! synthetic plan against temporary fixtures; nothing touches a real library.
use super::*;
use sha1::Digest;
use std::io::{Read, Write};

fn sha1_hex(bytes: &[u8]) -> String {
    sha1::Sha1::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn crc32_hex(bytes: &[u8]) -> String {
    let mut crc = 0xffff_ffffu32;
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
    format!("{:08x}", !crc)
}

#[derive(Clone)]
enum From<'a> {
    Loose,
    Zip(&'a Path, &'a str),
}

struct Member<'a> {
    target: &'a str,
    bytes: &'a [u8],
    from: From<'a>,
}

struct Lab {
    dir: tempfile::TempDir,
}

impl Lab {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("out")).unwrap();
        Self { dir }
    }
    fn root(&self) -> &Path {
        self.dir.path()
    }
    fn stage_root(&self) -> PathBuf {
        self.root().join("stage")
    }
    fn journal(&self) -> PathBuf {
        self.root().join("journal")
    }
    fn zip(&self, name: &str, entries: &[(&str, &[u8])]) -> PathBuf {
        let path = self.root().join(name);
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for (entry, bytes) in entries {
            writer
                .start_file(
                    *entry,
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Stored),
                )
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
        path
    }
    fn plan(&self, members: &[Member<'_>]) -> MameMergedReconstructionPlan {
        let loose_dir = self.root().join("loose-set");
        let mut required = Vec::new();
        let mut sources = Vec::new();
        for member in members {
            required.push(ReconstructionMemberRequirement {
                owner_set: "parent".into(),
                member_name: member.target.into(),
                size_bytes: Some(member.bytes.len() as u64),
                sha1: Some(sha1_hex(member.bytes)),
                crc32: Some(crc32_hex(member.bytes)),
            });
            let (archive_path, member_path, current_name, archive_identity) = match &member.from {
                From::Loose => {
                    std::fs::create_dir_all(&loose_dir).unwrap();
                    std::fs::write(loose_dir.join(member.target), member.bytes).unwrap();
                    (
                        loose_dir.clone(),
                        loose_dir.join(member.target),
                        member.target.to_string(),
                        None,
                    )
                }
                From::Zip(archive, entry) => (
                    archive.to_path_buf(),
                    PathBuf::from(entry),
                    entry.to_string(),
                    SourceArchiveIdentity::of(archive),
                ),
            };
            sources.push(ReconstructionMemberSource {
                archive_identity,
                archive_path,
                member_path,
                current_name,
                target_name: member.target.into(),
                observed_sha1: Some(sha1_hex(member.bytes)),
                observed_crc32: Some(crc32_hex(member.bytes)),
            });
        }
        required.sort_by(|a, b| a.member_name.cmp(&b.member_name));
        sources.sort_by(|a, b| a.target_name.cmp(&b.target_name));
        MameMergedReconstructionPlan {
            dat_version: "test".into(),
            dat_sha256: "digest".into(),
            parent: "parent".into(),
            clones: vec!["clone".into()],
            destination: self.root().join("out/parent.zip"),
            required_members: required,
            sources,
            missing_members: Vec::new(),
            duplicate_candidates: Vec::new(),
            hash_mismatches: Vec::new(),
            unresolved_ownership: Vec::new(),
            collisions: Vec::new(),
            ready_to_apply: true,
            reasons: Vec::new(),
        }
    }
    fn stage(&self, plan: &MameMergedReconstructionPlan) -> Result<PathBuf, String> {
        stage_reconstruction_output(plan, &self.stage_root())
    }
    fn nothing_was_staged_or_published(&self, plan: &MameMergedReconstructionPlan) {
        assert!(!plan.destination.exists(), "a failed stage published");
        let staged = self.stage_root().join("parent.zip.staged");
        assert!(!staged.exists(), "a half-built staging ZIP was left behind");
        assert!(!self.stage_root().join("source-members").exists());
        assert!(!self.journal().exists());
    }
}

fn read_back(zip_path: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(zip_path).unwrap()).unwrap();
    let mut out = std::collections::BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        out.insert(entry.name().to_string(), bytes);
    }
    out
}

fn patch(path: &Path, change: impl FnOnce(&mut Vec<u8>)) {
    let mut bytes = std::fs::read(path).unwrap();
    change(&mut bytes);
    std::fs::write(path, bytes).unwrap();
}

fn position(bytes: &[u8], needle: &[u8]) -> usize {
    bytes
        .windows(needle.len())
        .position(|w| w == needle)
        .unwrap()
}

// A. one required member from one ZIP
#[test]
fn a_single_member_is_staged_exactly_from_a_zip() {
    let lab = Lab::new();
    let zip = lab.zip(
        "source.zip",
        &[("pacman.6e", b"six-e bytes"), ("other", b"unrelated")],
    );
    let before = std::fs::read(&zip).unwrap();
    let plan = lab.plan(&[Member {
        target: "pacman.6e",
        bytes: b"six-e bytes",
        from: From::Zip(&zip, "pacman.6e"),
    }]);
    let staged = lab.stage(&plan).unwrap();
    assert_eq!(
        read_back(&staged),
        [("pacman.6e".to_string(), b"six-e bytes".to_vec())].into()
    );
    assert_eq!(
        std::fs::read(&zip).unwrap(),
        before,
        "the source is never written"
    );
    assert!(
        !lab.stage_root().join("source-members").exists(),
        "scratch removed"
    );
}

// B + C + D + E: several from one ZIP, several ZIPs, mixed with loose, nested source path
#[test]
fn members_come_from_one_zip_several_zips_and_loose_files_into_one_verified_set() {
    let lab = Lab::new();
    let one = lab.zip(
        "one.zip",
        &[
            ("deep/dir/pacman.6e", b"E"),
            ("pacman.6h", b"H"),
            ("junk", b"x"),
        ],
    );
    let two = lab.zip("two.zip", &[("pacman.6f", b"F")]);
    let plan = lab.plan(&[
        Member {
            target: "pacman.6e",
            bytes: b"E",
            from: From::Zip(&one, "deep/dir/pacman.6e"),
        },
        Member {
            target: "pacman.6f",
            bytes: b"F",
            from: From::Zip(&two, "pacman.6f"),
        },
        Member {
            target: "pacman.6h",
            bytes: b"H",
            from: From::Zip(&one, "pacman.6h"),
        },
        Member {
            target: "pacman.6j",
            bytes: b"J loose",
            from: From::Loose,
        },
    ]);
    let staged = lab.stage(&plan).unwrap();
    let files = read_back(&staged);
    assert_eq!(
        files.len(),
        4,
        "only the required members, none of the junk"
    );
    assert_eq!(
        files["pacman.6e"], b"E",
        "canonical name, source path dropped"
    );
    assert_eq!(files["pacman.6j"], b"J loose");
    verify_staged_output(&plan, &staged).unwrap();
}

// F. missing planned member, and P. a late failure publishes and leaves nothing
#[test]
fn a_missing_member_after_several_good_ones_stages_and_publishes_nothing() {
    let lab = Lab::new();
    let zip = lab.zip("source.zip", &[("a", b"A"), ("b", b"B"), ("c", b"C")]);
    let plan = lab.plan(&[
        Member {
            target: "a",
            bytes: b"A",
            from: From::Zip(&zip, "a"),
        },
        Member {
            target: "b",
            bytes: b"B",
            from: From::Zip(&zip, "b"),
        },
        Member {
            target: "c",
            bytes: b"C",
            from: From::Zip(&zip, "c"),
        },
        Member {
            target: "d",
            bytes: b"D",
            from: From::Zip(&zip, "d"),
        },
    ]);
    let error = lab.stage(&plan).unwrap_err();
    assert!(error.contains("missing"), "{error}");
    lab.nothing_was_staged_or_published(&plan);
    let error =
        apply_staged_reconstruction_output(&plan, &lab.stage_root(), &lab.journal()).unwrap_err();
    assert!(error.contains("missing"), "{error}");
    lab.nothing_was_staged_or_published(&plan);
}

// G. corrupt ZIP
#[test]
fn a_corrupt_or_truncated_zip_is_a_structured_failure_not_a_skip() {
    let lab = Lab::new();
    let zip = lab.zip("source.zip", &[("a", b"AAAA")]);
    let plan = lab.plan(&[Member {
        target: "a",
        bytes: b"AAAA",
        from: From::Zip(&zip, "a"),
    }]);
    patch(&zip, |bytes| bytes.truncate(bytes.len() / 2));
    let mut plan = plan;
    plan.sources[0].archive_identity = SourceArchiveIdentity::of(&zip);
    assert!(lab.stage(&plan).is_err());
    lab.nothing_was_staged_or_published(&plan);
}

// H. bad checksums: ALL authoritative values must agree
#[test]
fn a_wrong_crc_or_wrong_sha1_or_wrong_size_is_refused() {
    let lab = Lab::new();
    let zip = lab.zip("source.zip", &[("a", b"payload")]);
    let good = lab.plan(&[Member {
        target: "a",
        bytes: b"payload",
        from: From::Zip(&zip, "a"),
    }]);
    for tweak in 0..3 {
        let mut plan = good.clone();
        match tweak {
            0 => plan.required_members[0].crc32 = Some("deadbeef".into()),
            1 => plan.required_members[0].sha1 = Some("0".repeat(40)),
            _ => plan.required_members[0].size_bytes = Some(8),
        }
        let error = lab.stage(&plan).unwrap_err();
        assert!(error.contains("refused"), "case {tweak}: {error}");
        lab.nothing_was_staged_or_published(&plan);
    }
    // sanity: the unmodified plan stages
    lab.stage(&good).unwrap();
}

// I. source changed after the preview
#[test]
fn a_source_changed_after_the_preview_is_refused_even_when_size_and_time_match() {
    let lab = Lab::new();
    let zip = lab.zip("source.zip", &[("a", b"first!")]);
    let plan = lab.plan(&[Member {
        target: "a",
        bytes: b"first!",
        from: From::Zip(&zip, "a"),
    }]);
    // different size: caught by the recorded archive identity
    std::fs::remove_file(&zip).unwrap();
    lab.zip("source.zip", &[("a", b"first! and more")]);
    let error = lab.stage(&plan).unwrap_err();
    assert!(error.contains("changed since the preview"), "{error}");
    lab.nothing_was_staged_or_published(&plan);
    // same size, same mtime, different bytes: only the checksum can catch it
    let lab = Lab::new();
    let zip = lab.zip("source.zip", &[("a", b"first!")]);
    let plan = lab.plan(&[Member {
        target: "a",
        bytes: b"first!",
        from: From::Zip(&zip, "a"),
    }]);
    let mtime = std::fs::metadata(&zip).unwrap().modified().unwrap();
    std::fs::remove_file(&zip).unwrap();
    lab.zip("source.zip", &[("a", b"second")]);
    std::fs::File::options()
        .write(true)
        .open(&zip)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    assert_eq!(
        std::fs::metadata(&zip).unwrap().len(),
        plan.sources[0].archive_identity.unwrap().size_bytes
    );
    let error = lab.stage(&plan).unwrap_err();
    assert!(error.contains("refused"), "{error}");
    lab.nothing_was_staged_or_published(&plan);
}

// J. duplicate entry names
#[test]
fn duplicate_zip_entry_names_fail_closed_as_ambiguous() {
    let lab = Lab::new();
    let zip = lab.zip("source.zip", &[("dup.bin", b"AAAA"), ("zup.bin", b"BBBB")]);
    // rename the second entry to the same name as the first (same length)
    patch(&zip, |bytes| {
        while let Some(at) = bytes.windows(7).rposition(|w| w == b"zup.bin") {
            bytes[at..at + 7].copy_from_slice(b"dup.bin");
        }
    });
    let mut plan = lab.plan(&[Member {
        target: "dup.bin",
        bytes: b"AAAA",
        from: From::Zip(&zip, "dup.bin"),
    }]);
    plan.sources[0].archive_identity = SourceArchiveIdentity::of(&zip);
    let error = lab.stage(&plan).unwrap_err();
    assert!(error.contains("ambiguous"), "{error}");
    lab.nothing_was_staged_or_published(&plan);
}

// K + L. traversal and absolute names, backslashes, drive prefixes
#[test]
fn unsafe_entry_names_are_refused_and_nothing_escapes_staging() {
    for (index, name) in [
        "../pacman.6e",
        "/abs/pacman.6e",
        "..\\pacman.6e",
        "C:evil",
        "a/../../x",
    ]
    .into_iter()
    .enumerate()
    {
        let lab = Lab::new();
        let zip = lab.zip(&format!("s{index}.zip"), &[(name, b"data")]);
        let plan = lab.plan(&[Member {
            target: "pacman.6e",
            bytes: b"data",
            from: From::Zip(&zip, name),
        }]);
        let error = lab.stage(&plan).unwrap_err();
        assert!(error.contains("unsafe"), "{name}: {error}");
        lab.nothing_was_staged_or_published(&plan);
        assert!(
            !lab.root().join("pacman.6e").exists()
                && !lab.root().parent().unwrap().join("pacman.6e").exists()
        );
    }
}

// M. symlink / special entries
#[test]
fn symlink_and_special_entries_are_never_materialised() {
    let lab = Lab::new();
    let path = lab.root().join("links.zip");
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    writer
        .add_symlink(
            "pacman.6e",
            "/etc/passwd",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    writer.finish().unwrap();
    let plan = lab.plan(&[Member {
        target: "pacman.6e",
        bytes: b"/etc/passwd",
        from: From::Zip(&path, "pacman.6e"),
    }]);
    let error = lab.stage(&plan).unwrap_err();
    assert!(error.contains("not a regular file"), "{error}");
    lab.nothing_was_staged_or_published(&plan);
    // a FIFO/device mode on a regular-looking entry is refused the same way
    for mode in [0o010644u32, 0o020644, 0o060644, 0o140644] {
        let lab = Lab::new();
        let path = lab.zip("special.zip", &[("pacman.6e", b"data")]);
        patch(&path, |bytes| {
            let central = position(bytes, b"PK\x01\x02");
            bytes[central + 5] = 3; // made by Unix
            bytes[central + 38..central + 42].copy_from_slice(&(mode << 16).to_le_bytes());
        });
        let plan = lab.plan(&[Member {
            target: "pacman.6e",
            bytes: b"data",
            from: From::Zip(&path, "pacman.6e"),
        }]);
        let error = lab.stage(&plan).unwrap_err();
        assert!(error.contains("not a regular file"), "{mode:o}: {error}");
        lab.nothing_was_staged_or_published(&plan);
    }
}

// N. encrypted / unsupported compression
#[test]
fn encrypted_and_unsupported_compression_entries_are_refused() {
    let lab = Lab::new();
    let zip = lab.zip("source.zip", &[("a", b"data")]);
    let mut plan = lab.plan(&[Member {
        target: "a",
        bytes: b"data",
        from: From::Zip(&zip, "a"),
    }]);
    patch(&zip, |bytes| {
        let central = position(bytes, b"PK\x01\x02");
        bytes[central + 8] |= 1;
        let local = position(bytes, b"PK\x03\x04");
        bytes[local + 6] |= 1;
    });
    plan.sources[0].archive_identity = SourceArchiveIdentity::of(&zip);
    assert!(
        lab.stage(&plan)
            .unwrap_err()
            .to_lowercase()
            .contains("encrypted")
    );
    lab.nothing_was_staged_or_published(&plan);

    let lab = Lab::new();
    let zip = lab.zip("source.zip", &[("a", b"data")]);
    let mut plan = lab.plan(&[Member {
        target: "a",
        bytes: b"data",
        from: From::Zip(&zip, "a"),
    }]);
    patch(&zip, |bytes| {
        let central = position(bytes, b"PK\x01\x02");
        bytes[central + 10] = 99;
        let local = position(bytes, b"PK\x03\x04");
        bytes[local + 8] = 99;
    });
    plan.sources[0].archive_identity = SourceArchiveIdentity::of(&zip);
    assert!(lab.stage(&plan).is_err());
    lab.nothing_was_staged_or_published(&plan);
}

// O. duplicate destination member names are refused by the planner
#[test]
fn two_different_payloads_for_one_destination_name_block_the_plan() {
    use crate::dat::model::{
        DatEcosystem, DatFormat, DatGameEntry, DatPackingPolicy, DatRomEntry, DatSource,
    };
    let rom = |name: &str, bytes: &[u8]| DatRomEntry {
        name: name.into(),
        sha1: Some(sha1_hex(bytes)),
        crc32: Some(crc32_hex(bytes)),
        size_bytes: Some(bytes.len() as u64),
        ..Default::default()
    };
    let games = vec![
        DatGameEntry {
            name: "parent".into(),
            roms: vec![rom("same.bin", b"one")],
            ..Default::default()
        },
        DatGameEntry {
            name: "clone".into(),
            clone_of: Some("parent".into()),
            roms: vec![rom("same.bin", b"two")],
            ..Default::default()
        },
    ];
    let parsed = ParsedDat {
        source: DatSource {
            format: DatFormat::Logiqx,
            ecosystem: DatEcosystem::MAMEArcade,
            file_path: "fixture".into(),
            name: None,
            description: None,
            version: Some("test".into()),
            author: None,
            homepage: None,
            clrmamepro_header: None,
            entry_count: 2,
            rom_count: 2,
            parse_warnings: vec![],
            packing_policy: DatPackingPolicy::Standard,
        },
        games,
    };
    let lab = Lab::new();
    let plan =
        build_merged_reconstruction_plan(lab.root(), &parsed, &[], "parent", "digest").unwrap();
    assert!(
        plan.collisions
            .iter()
            .any(|c| c.contains("duplicate destination member name")),
        "{:?}",
        plan.collisions
    );
    assert!(!plan.ready_to_apply);
    // and an unsafe destination name is refused too
    assert!(
        !is_plain_member_name("../x")
            && !is_plain_member_name("/x")
            && !is_plain_member_name("a\\b")
    );
    assert!(is_plain_member_name("sub/dir.bin"));
}

// Disk space is checked before a single byte is staged.
#[test]
fn insufficient_free_space_fails_before_any_extraction() {
    let lab = Lab::new();
    let zip = lab.zip("source.zip", &[("a", b"data")]);
    let plan = lab.plan(&[Member {
        target: "a",
        bytes: b"data",
        from: From::Zip(&zip, "a"),
    }]);
    let error =
        stage_reconstruction_output_with(&plan, &lab.stage_root(), &|_| Some(10)).unwrap_err();
    assert!(error.contains("not enough free space"), "{error}");
    assert!(!lab.stage_root().exists(), "nothing was created");
    assert!(
        stage_reconstruction_output_with(&plan, &lab.stage_root(), &|_| Some(u64::MAX)).is_ok()
    );
    assert!(free_space_bytes(lab.root()).is_some_and(|free| free > 0));
}

// Q. publish failure reuses the existing executor and leaves everything as it was
#[test]
fn a_publish_collision_changes_nothing_and_the_sources_stay_intact() {
    let lab = Lab::new();
    let zip = lab.zip("source.zip", &[("a", b"AAA")]);
    let before = std::fs::read(&zip).unwrap();
    let plan = lab.plan(&[Member {
        target: "a",
        bytes: b"AAA",
        from: From::Zip(&zip, "a"),
    }]);
    std::fs::write(&plan.destination, b"someone else's file").unwrap();
    let error =
        apply_staged_reconstruction_output(&plan, &lab.stage_root(), &lab.journal()).unwrap_err();
    assert!(
        error.to_lowercase().contains("collision") || error.contains("refused"),
        "{error}"
    );
    assert_eq!(
        std::fs::read(&plan.destination).unwrap(),
        b"someone else's file"
    );
    assert_eq!(std::fs::read(&zip).unwrap(), before);
}

// R + S + T. successful publish verifies every member; undo works for packed and mixed
#[test]
fn packed_and_mixed_reconstructions_publish_verify_and_undo_identically() {
    for mixed in [false, true] {
        let lab = Lab::new();
        let one = lab.zip("one.zip", &[("x/a.bin", b"alpha"), ("b.bin", b"bravo")]);
        let two = lab.zip("two.zip", &[("c.bin", b"charlie")]);
        let members = [
            Member {
                target: "a.bin",
                bytes: b"alpha",
                from: From::Zip(&one, "x/a.bin"),
            },
            Member {
                target: "b.bin",
                bytes: b"bravo",
                from: From::Zip(&one, "b.bin"),
            },
            Member {
                target: "c.bin",
                bytes: b"charlie",
                from: if mixed {
                    From::Loose
                } else {
                    From::Zip(&two, "c.bin")
                },
            },
        ];
        let plan = lab.plan(&members);
        let (one_before, two_before) = (std::fs::read(&one).unwrap(), std::fs::read(&two).unwrap());
        let outcome =
            apply_staged_reconstruction_output(&plan, &lab.stage_root(), &lab.journal()).unwrap();
        // R: the published set holds exactly the required members, byte for byte
        let published = read_back(&plan.destination);
        assert_eq!(published.len(), 3);
        for member in &members {
            assert_eq!(published[member.target], member.bytes, "{}", member.target);
        }
        verify_staged_output(&plan, &plan.destination).unwrap();
        assert_eq!(std::fs::read(&one).unwrap(), one_before);
        assert_eq!(std::fs::read(&two).unwrap(), two_before);
        // S/T: the existing journalled undo, unaware of how the bytes were packaged
        let mut transaction = outcome.transaction;
        crate::dat::rename_apply::rollback_transaction(
            &mut transaction,
            &lab.journal(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(!plan.destination.exists(), "undo removed the published set");
        assert_eq!(std::fs::read(&one).unwrap(), one_before);
        assert_eq!(std::fs::read(&two).unwrap(), two_before);
    }
}

// Streaming: a large member is never buffered whole.
#[test]
fn a_large_member_is_streamed_with_bounded_memory() {
    // Peak RSS is per process, so other parallel tests would pollute it: the
    // measurement runs alone in a child copy of this test binary.
    if std::env::var_os("EMUWIZ_PACKED_STREAM_CHILD").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "dat::mame_merged_reconstruction::packed_tests::a_large_member_is_streamed_with_bounded_memory",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("EMUWIZ_PACKED_STREAM_CHILD", "1")
            .output()
            .unwrap();
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        return;
    }
    fn peak_kib() -> u64 {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find_map(|line| line.strip_prefix("VmHWM:"))
                    .and_then(|rest| rest.split_whitespace().next().and_then(|n| n.parse().ok()))
            })
            .unwrap_or(0)
    }
    const SIZE: usize = 32 * 1024 * 1024;
    let lab = Lab::new();
    let path = lab.root().join("big.zip");
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    writer
        .start_file(
            "big.rom",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored)
                .large_file(true),
        )
        .unwrap();
    let mut chunk = vec![0u8; 1024 * 1024];
    let mut state = 0x1234_5678_9abc_def1u64;
    let mut sha = sha1::Sha1::new();
    for _ in 0..SIZE / chunk.len() {
        for byte in chunk.iter_mut() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *byte = state as u8;
        }
        sha.update(&chunk);
        writer.write_all(&chunk).unwrap();
    }
    writer.finish().unwrap();
    let sha1: String = sha.finalize().iter().map(|b| format!("{b:02x}")).collect();
    let mut plan = lab.plan(&[Member {
        target: "small",
        bytes: b"x",
        from: From::Loose,
    }]);
    plan.required_members = vec![ReconstructionMemberRequirement {
        owner_set: "parent".into(),
        member_name: "big.rom".into(),
        size_bytes: Some(SIZE as u64),
        sha1: Some(sha1.clone()),
        crc32: None,
    }];
    plan.sources = vec![ReconstructionMemberSource {
        archive_identity: SourceArchiveIdentity::of(&path),
        archive_path: path.clone(),
        member_path: PathBuf::from("big.rom"),
        current_name: "big.rom".into(),
        target_name: "big.rom".into(),
        observed_sha1: Some(sha1),
        observed_crc32: None,
    }];
    let before = peak_kib();
    let started = std::time::Instant::now();
    let staged = lab.stage(&plan).unwrap();
    let elapsed = started.elapsed();
    let growth_mib = peak_kib().saturating_sub(before) / 1024;
    eprintln!(
        "streamed a {} MiB member in {elapsed:?}; peak RSS grew {growth_mib} MiB",
        SIZE / 1024 / 1024
    );
    assert_eq!(
        std::fs::metadata(&staged).unwrap().len() > SIZE as u64,
        true
    );
    // buffering the whole member would add at least its size to the peak
    assert!(
        growth_mib < (SIZE / 1024 / 1024 * 3 / 4) as u64,
        "peak grew by {growth_mib} MiB"
    );
}

/// Real MAME bytes and the real 0.174 DAT, read-only, staged into a scratch
/// directory (never the library):
/// `MAME_DAT=/path/MAME.0.174.Arcade.XML.dat MAME_ROOT=/mnt/usbdrive/games/arcade \
///  cargo test -p archivefs-core --lib -- --ignored real_mame_bytes_stage_from_a_scratch_zip --nocapture`
#[test]
#[ignore]
fn real_mame_bytes_stage_from_a_scratch_zip() {
    let dat_path = PathBuf::from(std::env::var("MAME_DAT").unwrap());
    let root = PathBuf::from(std::env::var("MAME_ROOT").unwrap());
    let dat = crate::dat::mame_arcade_join::load_verified_mame_0174(&dat_path).unwrap();
    let mut proved = 0;
    for set in [
        "pacman", "puckman", "galaga", "1942", "mslug", "dkong", "bublbobl",
    ] {
        let Some(game) = dat.parsed.games.iter().find(|game| game.name == set) else {
            continue;
        };
        let physical: Vec<_> = game
            .roms
            .iter()
            .filter(|rom| !is_non_physical(rom))
            .collect();
        // every ROM file must exist in the real extracted set (read-only)
        let mut real = Vec::new();
        for rom in &physical {
            match std::fs::read(root.join(set).join(&rom.name)) {
                Ok(bytes) => real.push((rom, bytes)),
                Err(_) => break,
            }
        }
        if physical.is_empty() || real.len() != physical.len() {
            eprintln!("{set}: not fully present as loose files here; skipped");
            continue;
        }
        let lab = Lab::new();
        // a scratch ZIP holding the real bytes under nested source paths
        let entries: Vec<(String, &[u8])> = real
            .iter()
            .map(|(rom, bytes)| (format!("nested/{}", rom.name), bytes.as_slice()))
            .collect();
        let borrowed: Vec<(&str, &[u8])> = entries.iter().map(|(n, b)| (n.as_str(), *b)).collect();
        let zip = lab.zip("scratch-source.zip", &borrowed);
        let mut plan = lab.plan(&[Member {
            target: "placeholder",
            bytes: b"x",
            from: From::Loose,
        }]);
        plan.parent = set.to_string();
        plan.destination = lab.root().join(format!("out/{set}.zip"));
        plan.required_members.clear();
        plan.sources.clear();
        for (rom, _) in &real {
            plan.required_members.push(ReconstructionMemberRequirement {
                owner_set: set.into(),
                member_name: rom.name.clone(),
                size_bytes: rom.size_bytes,
                sha1: rom.sha1.clone().map(|v| v.to_ascii_lowercase()),
                crc32: rom.crc32.clone().map(|v| v.to_ascii_lowercase()),
            });
            plan.sources.push(ReconstructionMemberSource {
                archive_identity: SourceArchiveIdentity::of(&zip),
                archive_path: zip.clone(),
                member_path: PathBuf::from(format!("nested/{}", rom.name)),
                current_name: format!("nested/{}", rom.name),
                target_name: rom.name.clone(),
                observed_sha1: rom.sha1.clone(),
                observed_crc32: rom.crc32.clone(),
            });
        }
        let started = std::time::Instant::now();
        let outcome =
            apply_staged_reconstruction_output(&plan, &lab.stage_root(), &lab.journal()).unwrap();
        let published = read_back(&plan.destination);
        assert_eq!(published.len(), real.len());
        for (rom, bytes) in &real {
            assert_eq!(&published[&rom.name], bytes, "{set}/{}", rom.name);
            assert_eq!(
                Some(sha1_hex(&published[&rom.name])),
                rom.sha1.clone().map(|v| v.to_ascii_lowercase())
            );
        }
        verify_staged_output(&plan, &plan.destination).unwrap();
        let mut transaction = outcome.transaction;
        crate::dat::rename_apply::rollback_transaction(
            &mut transaction,
            &lab.journal(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(!plan.destination.exists());
        eprintln!(
            "{set}: {} members staged from nested paths in a scratch ZIP, every DAT SHA-1 verified, published to scratch and undone in {:?}",
            real.len(),
            started.elapsed()
        );
        proved += 1;
    }
    assert!(proved > 0, "no real set was fully present");
}

// ---- receipt provenance and the remaining failure matrix ----------------------------

fn provenance_of(
    transaction: &crate::dat::rename_apply::model::RenameTransaction,
) -> serde_json::Value {
    transaction
        .unknown
        .get(MAME_RECONSTRUCTION_PROVENANCE_KEY)
        .cloned()
        .expect("the receipt carries source provenance")
}

#[test]
fn the_receipt_records_where_every_member_came_from_and_what_was_actually_staged() {
    let lab = Lab::new();
    let one = lab.zip("one.zip", &[("x/a.bin", b"alpha"), ("b.bin", b"bravo")]);
    let plan = lab.plan(&[
        Member {
            target: "a.bin",
            bytes: b"alpha",
            from: From::Zip(&one, "x/a.bin"),
        },
        Member {
            target: "b.bin",
            bytes: b"bravo",
            from: From::Zip(&one, "b.bin"),
        },
        Member {
            target: "c.bin",
            bytes: b"charlie",
            from: From::Loose,
        },
    ]);
    let outcome =
        apply_staged_reconstruction_output(&plan, &lab.stage_root(), &lab.journal()).unwrap();
    let provenance = provenance_of(&outcome.transaction);
    assert_eq!(provenance["dat_sha256"], "digest");
    assert_eq!(provenance["parent"], "parent");
    let members = provenance["members"].as_array().unwrap();
    assert_eq!(members.len(), 3);
    let by_name = |name: &str| {
        members
            .iter()
            .find(|member| member["target_name"] == name)
            .unwrap_or_else(|| panic!("no provenance for {name}"))
    };
    // A packed member names its archive AND the entry inside it.
    let alpha = by_name("a.bin");
    assert_eq!(alpha["packed"], true);
    assert_eq!(alpha["source_archive"], one.display().to_string());
    assert_eq!(alpha["source_member"], "x/a.bin");
    // A loose member is recorded as such.
    assert_eq!(by_name("c.bin")["packed"], false);
    // Expected and actually staged hashes are both recorded, and agree.
    for member in members {
        assert_eq!(member["expected"]["sha1"], member["staged"]["sha1"]);
        assert_eq!(member["expected"]["crc32"], member["staged"]["crc32"]);
        assert_eq!(
            member["expected"]["size_bytes"],
            member["staged"]["size_bytes"]
        );
        assert_eq!(member["owner_set"], "parent");
        assert!(
            member["ownership"]["basis"]
                .as_str()
                .unwrap()
                .contains("checksum"),
            "ownership is by checksum, never by filename"
        );
        assert_eq!(
            member["ownership"]["observed_sha1"],
            member["expected"]["sha1"]
        );
    }
    assert_eq!(alpha["staged"]["sha1"], sha1_hex(b"alpha"));
    // The same provenance survives a reload from the journal directory, which
    // is how the history list shows a receipt after a restart.
    let (reloaded, problems) = crate::dat::rename_apply::list_journals(&lab.journal());
    assert!(problems.is_empty(), "{problems:?}");
    let reloaded = reloaded
        .into_iter()
        .find(|transaction| transaction.transaction_id == outcome.transaction.transaction_id)
        .expect("the receipt is in the journal directory");
    assert_eq!(provenance_of(&reloaded), provenance);
}

#[test]
fn a_source_archive_that_disappears_after_the_preview_is_refused_and_nothing_is_staged() {
    let lab = Lab::new();
    let zip = lab.zip("source.zip", &[("a", b"AAA")]);
    let plan = lab.plan(&[Member {
        target: "a",
        bytes: b"AAA",
        from: From::Zip(&zip, "a"),
    }]);
    std::fs::remove_file(&zip).unwrap();
    let error = lab.stage(&plan).unwrap_err();
    assert!(
        error.contains("changed since the preview") || error.contains("refused"),
        "{error}"
    );
    lab.nothing_was_staged_or_published(&plan);
}

#[test]
fn unrelated_entries_in_a_source_archive_are_never_decoded() {
    let lab = Lab::new();
    let zip = lab.zip(
        "source.zip",
        &[("wanted", b"good bytes"), ("decoy", b"decoy bytes!")],
    );
    // Corrupt the decoy's stored data. If staging decoded or hashed the whole
    // archive it would trip over this; only the requested entry may be read.
    patch(&zip, |bytes| {
        let at = position(bytes, b"decoy bytes!");
        bytes[at] ^= 0xff;
    });
    // Guard: a whole-archive verification really would notice the damage.
    let whole = read_zip_evidence(&zip).unwrap();
    assert!(
        whole.iter().any(|member| !matches!(
            member.status,
            crate::dat::archive::ArchiveMemberStatus::HashComplete
        )),
        "the decoy corruption must be detectable, or this test proves nothing"
    );
    let plan = lab.plan(&[Member {
        target: "wanted",
        bytes: b"good bytes",
        from: From::Zip(&zip, "wanted"),
    }]);
    let staged = lab.stage(&plan).unwrap();
    assert_eq!(
        read_back(&staged),
        [("wanted".to_string(), b"good bytes".to_vec())].into(),
        "only the requested member was extracted"
    );
}

#[test]
fn a_reconstruction_that_fails_verification_after_extraction_leaves_nothing_behind() {
    let lab = Lab::new();
    let zip = lab.zip("source.zip", &[("a", b"AAA"), ("b", b"BBB")]);
    let mut plan = lab.plan(&[
        Member {
            target: "a",
            bytes: b"AAA",
            from: From::Zip(&zip, "a"),
        },
        Member {
            target: "b",
            bytes: b"BBB",
            from: From::Zip(&zip, "b"),
        },
    ]);
    // A requirement with no source: every member extracts fine, but the staged
    // set cannot be the complete reviewed set, so verification must refuse it.
    plan.required_members.push(ReconstructionMemberRequirement {
        owner_set: "parent".into(),
        member_name: "unsourced".into(),
        size_bytes: Some(3),
        sha1: Some(sha1_hex(b"???")),
        crc32: Some(crc32_hex(b"???")),
    });
    let error = lab.stage(&plan).unwrap_err();
    assert!(
        error.contains("inventory") || error.contains("mismatch"),
        "{error}"
    );
    lab.nothing_was_staged_or_published(&plan);
}

#[test]
fn a_packed_source_changed_between_review_and_apply_is_refused_without_replanning() {
    let lab = Lab::new();
    let zip = lab.zip("source.zip", &[("a", b"AAA")]);
    let plan = lab.plan(&[Member {
        target: "a",
        bytes: b"AAA",
        from: From::Zip(&zip, "a"),
    }]);
    let reviewed = review_reconstruction_publication(&plan, None).unwrap();
    // The archive is replaced after the review. Apply must not rescan, replan
    // or quietly use the new contents.
    std::fs::remove_file(&zip).unwrap();
    lab.zip("source.zip", &[("a", b"AAA"), ("extra", b"more")]);
    let error = reviewed
        .apply(&lab.stage_root(), &lab.journal())
        .unwrap_err();
    assert!(
        error.contains("stale") || error.contains("review again"),
        "{error}"
    );
    lab.nothing_was_staged_or_published(&plan);
}

#[test]
fn a_second_packed_member_failing_leaves_the_first_unpublished_and_the_source_intact() {
    let lab = Lab::new();
    let zip = lab.zip("source.zip", &[("a", b"AAA"), ("b", b"BBB")]);
    let before = std::fs::read(&zip).unwrap();
    let mut plan = lab.plan(&[
        Member {
            target: "a",
            bytes: b"AAA",
            from: From::Zip(&zip, "a"),
        },
        Member {
            target: "b",
            bytes: b"BBB",
            from: From::Zip(&zip, "b"),
        },
    ]);
    // The second source claims an entry the archive does not hold.
    plan.sources[1].current_name = "not-in-the-archive".into();
    let error =
        apply_staged_reconstruction_output(&plan, &lab.stage_root(), &lab.journal()).unwrap_err();
    assert!(
        error.contains("not-in-the-archive") || error.contains("refused"),
        "{error}"
    );
    lab.nothing_was_staged_or_published(&plan);
    assert_eq!(std::fs::read(&zip).unwrap(), before);
}
