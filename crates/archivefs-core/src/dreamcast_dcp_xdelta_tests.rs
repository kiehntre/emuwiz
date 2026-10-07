//! DCP xdelta application tests. Tiny synthetic trees in temp directories; no
//! real game path is ever touched. Failure paths use fake `xdelta3` scripts
//! (structured argv: `-d -s <base> <patch> <out>`); the happy path also runs
//! the real tool when it is installed.

use super::tests::{binding, ip_bin, package};
use super::*;
use crate::patch_output_recovery::tree::{TreePatchState, inspect, publish, undo};
use std::os::unix::fs::PermissionsExt;

const VCDIFF: &[u8] = &[0xd6, 0xc3, 0xc4, 0x00, 1, 2, 3];

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"-V\" ]; then echo faketool-1.0 >&2; exit 0; fi\n{body}\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}
/// Output is `DELTA:` + base bytes.
fn good_tool(dir: &Path) -> PathBuf {
    script(
        dir,
        "good-xdelta3",
        "printf 'DELTA:' > \"$5\"; cat \"$3\" >> \"$5\"",
    )
}
fn failing_tool(dir: &Path) -> PathBuf {
    script(dir, "failing-xdelta3", "echo bad base >&2; exit 3")
}

struct World {
    temp: tempfile::TempDir,
    source: PathBuf,
    patch: PathBuf,
    destination: PathBuf,
}
impl World {
    fn new(members: &[(&str, &[u8])]) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir_all(source.join("bootsector")).unwrap();
        fs::write(source.join("bootsector/IP.BIN"), ip_bin()).unwrap();
        fs::write(source.join("1ST_READ.BIN"), b"old boot").unwrap();
        fs::write(source.join("a.bin"), b"base a").unwrap();
        fs::write(source.join("b.bin"), b"base b").unwrap();
        fs::write(source.join("untouched"), b"retain").unwrap();
        let patch = temp.path().join("patch.dcp");
        package(&patch, members);
        let destination = temp.path().join("published");
        Self {
            temp,
            source,
            patch,
            destination,
        }
    }
    fn review(&self, tool: Option<&Path>) -> io::Result<DreamcastDcpPlan> {
        review_dreamcast_dcp_with_tool(
            &self.source,
            &self.patch,
            &self.destination,
            &binding(&self.source, &self.patch),
            tool,
        )
    }
    fn tool_dir(&self) -> PathBuf {
        let dir = self.temp.path().join("tools");
        fs::create_dir_all(&dir).unwrap();
        dir
    }
    fn source_snapshot(&self) -> Contents {
        Contents::read(&self.source, MAX_SOURCE_BYTES).unwrap()
    }
}
fn refusal(result: io::Result<impl Sized>) -> DreamcastDeltaRefusal {
    let error = result.err().expect("expected a refusal");
    delta_refusal(&error)
        .unwrap_or_else(|| panic!("not a typed refusal: {error}"))
        .clone()
}

#[test]
fn valid_delta_applies_to_the_exact_base_and_publishes_with_a_receipt() {
    let world = World::new(&[("a.bin.xdelta", VCDIFF)]);
    let tool = good_tool(&world.tool_dir());
    let before = world.source_snapshot();
    let plan = world.review(Some(&tool)).unwrap();
    let prepared = plan.prepare().unwrap();
    assert_eq!(
        inspect(&prepared.journal_path).unwrap(),
        TreePatchState::Staged
    );
    assert!(!world.destination.exists());
    publish(&prepared.journal_path).unwrap();
    assert_eq!(
        fs::read(world.destination.join("a.bin")).unwrap(),
        b"DELTA:base a"
    );
    assert_eq!(
        fs::read(world.destination.join("b.bin")).unwrap(),
        b"base b"
    );
    assert!(!world.destination.join("a.bin.xdelta").exists());

    // Receipt: package, member, base and output hashes, tool, source tree.
    let receipt = plan.delta_receipt().unwrap();
    assert_eq!(receipt.package_sha256, plan.package.package_sha256);
    assert_eq!(
        receipt.source_tree_sha256,
        source_tree_sha256(&world.source).unwrap()
    );
    assert_eq!(receipt.tool_identity.as_deref(), Some("faketool-1.0"));
    let entry = &receipt.entries[0];
    assert_eq!(entry.member, "a.bin.xdelta");
    assert_eq!(entry.base_sha256, digest(b"base a"));
    assert_eq!(entry.output_sha256, digest(b"DELTA:base a"));
    assert_eq!(entry.claim, DreamcastDeltaClaim::DecodedFromExactBase);
    let sidecar = prepared.journal_path.with_extension("dcp-delta.json");
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(sidecar).unwrap()).unwrap();
    assert_eq!(saved["entries"][0]["claim"], "decoded_from_exact_base");

    // Undo restores the exact pre-operation state; the source never changed.
    undo(&prepared.journal_path).unwrap();
    assert!(!world.destination.exists());
    assert_eq!(world.source_snapshot(), before);
}

#[test]
fn real_xdelta3_round_trip() {
    let Some(tool) = locate_xdelta3() else {
        eprintln!("xdelta3 not installed; skipping");
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let new_a = b"base a with a considerably longer, edited tail".to_vec();
    fs::write(temp.path().join("old"), b"base a").unwrap();
    fs::write(temp.path().join("new"), &new_a).unwrap();
    let status = std::process::Command::new(&tool)
        .args(["-e", "-s"])
        .arg(temp.path().join("old"))
        .arg(temp.path().join("new"))
        .arg(temp.path().join("delta"))
        .status()
        .unwrap();
    assert!(status.success());
    let delta = fs::read(temp.path().join("delta")).unwrap();
    let world = World::new(&[("a.bin.xdelta", &delta)]);
    let prepared = world.review(None).unwrap().prepare().unwrap();
    publish(&prepared.journal_path).unwrap();
    assert_eq!(fs::read(world.destination.join("a.bin")).unwrap(), new_a);
}

#[test]
fn real_xdelta3_rejects_the_wrong_base() {
    let Some(tool) = locate_xdelta3() else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join("old"),
        b"completely different base content",
    )
    .unwrap();
    fs::write(
        temp.path().join("new"),
        b"completely different base content!",
    )
    .unwrap();
    std::process::Command::new(&tool)
        .args(["-e", "-s"])
        .arg(temp.path().join("old"))
        .arg(temp.path().join("new"))
        .arg(temp.path().join("delta"))
        .status()
        .unwrap();
    let delta = fs::read(temp.path().join("delta")).unwrap();
    let world = World::new(&[("a.bin.xdelta", &delta)]); // a.bin is "base a"
    let plan = world.review(None).unwrap();
    let error = plan.prepare().unwrap_err();
    assert!(error.to_string().contains("xdelta3"), "{error}");
    assert!(!world.destination.exists());
}

#[test]
fn tool_failure_publishes_nothing() {
    let world = World::new(&[("a.bin.xdelta", VCDIFF)]);
    let tool = failing_tool(&world.tool_dir());
    let before = world.source_snapshot();
    let plan = world.review(Some(&tool)).unwrap();
    let error = plan.prepare().unwrap_err();
    assert!(error.to_string().contains("xdelta3 failed"), "{error}");
    assert!(!world.destination.exists());
    assert_eq!(world.source_snapshot(), before);
    assert!(plan.delta_receipt().is_none());
}

#[test]
fn missing_and_ambiguous_bases_are_refused() {
    let world = World::new(&[("nothere.bin.xdelta", VCDIFF)]);
    let tool = good_tool(&world.tool_dir());
    assert!(matches!(
        refusal(world.review(Some(&tool))),
        DreamcastDeltaRefusal::MissingBase { .. }
    ));
    let world = World::new(&[("A.BIN.xdelta", VCDIFF)]);
    let tool = good_tool(&world.tool_dir());
    assert!(matches!(
        refusal(world.review(Some(&tool))),
        DreamcastDeltaRefusal::AmbiguousBase { candidates, .. } if candidates == ["a.bin"]
    ));
}

#[test]
fn a_base_changed_after_preview_refuses_apply() {
    let world = World::new(&[("a.bin.xdelta", VCDIFF)]);
    let tool = good_tool(&world.tool_dir());
    let plan = world.review(Some(&tool)).unwrap();
    fs::write(world.source.join("a.bin"), b"replaced").unwrap();
    assert!(plan.prepare().is_err());
    assert!(!world.destination.exists());
    // Reached directly (the shared tree freshness check normally trips first),
    // the delta binding itself refuses with a typed reason.
    let staging = world.temp.path().join("stage");
    fs::create_dir(&staging).unwrap();
    assert!(matches!(
        refusal(plan.produce(&staging)),
        DreamcastDeltaRefusal::BaseChanged { .. }
    ));
}

#[test]
fn traversal_and_unsafe_member_paths_are_refused() {
    for name in [
        "../a.bin.xdelta",
        "/abs.xdelta",
        "a\\b.xdelta",
        "x/../a.bin.xdelta",
    ] {
        let world = World::new(&[(name, VCDIFF)]);
        assert!(inspect_dreamcast_dcp(&world.patch).is_err(), "{name}");
    }
}

#[test]
fn direct_replacement_ip_bin_and_delta_share_one_transaction() {
    let mut ip = ip_bin();
    ip[128] = b'X';
    let world = World::new(&[
        ("1ST_READ.BIN", b"new boot"),
        ("bootsector/IP.BIN", &ip),
        ("a.bin.xdelta", VCDIFF),
    ]);
    let tool = good_tool(&world.tool_dir());
    let prepared = world.review(Some(&tool)).unwrap().prepare().unwrap();
    publish(&prepared.journal_path).unwrap();
    assert_eq!(
        fs::read(world.destination.join("1ST_READ.BIN")).unwrap(),
        b"new boot"
    );
    assert_eq!(
        fs::read(world.destination.join("bootsector/IP.BIN")).unwrap(),
        ip
    );
    assert_eq!(
        fs::read(world.destination.join("a.bin")).unwrap(),
        b"DELTA:base a"
    );
    assert_eq!(
        fs::read(world.destination.join("untouched")).unwrap(),
        b"retain"
    );
}

#[test]
fn a_second_failing_delta_publishes_nothing_even_though_the_first_staged() {
    let world = World::new(&[
        ("a.bin.xdelta", VCDIFF),
        ("b.bin.xdelta", VCDIFF),
        ("1ST_READ.BIN", b"new boot"),
    ]);
    // Succeeds for a.bin, fails for b.bin.
    let tool = script(
        &world.tool_dir(),
        "second-fails",
        "case \"$3\" in *b.bin) exit 3;; esac\nprintf 'DELTA:' > \"$5\"; cat \"$3\" >> \"$5\"",
    );
    let before = world.source_snapshot();
    let plan = world.review(Some(&tool)).unwrap();
    assert!(plan.prepare().is_err());
    assert!(!world.destination.exists());
    assert_eq!(world.source_snapshot(), before);
    assert!(plan.delta_receipt().is_none());
    assert!(!world.patch.with_extension("dcp-delta.json").exists());
}

#[test]
fn collisions_are_refused_with_typed_reasons() {
    let tool_for = |w: &World| good_tool(&w.tool_dir());
    let world = World::new(&[("a.bin.xdelta", VCDIFF), ("a.bin.vcdiff", VCDIFF)]);
    assert!(matches!(
        refusal(world.review(Some(&tool_for(&world)))),
        DreamcastDeltaRefusal::DuplicateDestination { .. }
    ));
    let world = World::new(&[("a.bin", b"direct"), ("a.bin.xdelta", VCDIFF)]);
    assert!(matches!(
        refusal(world.review(Some(&tool_for(&world)))),
        DreamcastDeltaRefusal::DirectAndDeltaCollision { .. }
    ));
}

#[test]
fn unsupported_suffix_ip_bin_target_non_vcdiff_and_new_files_are_refused() {
    let cases: Vec<(Vec<(&str, &[u8])>, fn(&DreamcastDeltaRefusal) -> bool)> = vec![
        (vec![("untouched.ips", b"PATCH")], |r| {
            matches!(r, DreamcastDeltaRefusal::UnsupportedDeltaSuffix { .. })
        }),
        (vec![("dir/.xdelta", VCDIFF)], |r| {
            matches!(r, DreamcastDeltaRefusal::UnsupportedDeltaSuffix { .. })
        }),
        (vec![("bootsector/IP.BIN.xdelta", VCDIFF)], |r| {
            matches!(r, DreamcastDeltaRefusal::DeltaTargetsIpBin { .. })
        }),
        (vec![("a.bin.xdelta", b"not a vcdiff")], |r| {
            matches!(r, DreamcastDeltaRefusal::NotVcdiff { .. })
        }),
        (vec![("brand-new.bin", b"x")], |r| {
            matches!(r, DreamcastDeltaRefusal::NewFileNotSupported { .. })
        }),
    ];
    for (members, check) in cases {
        let world = World::new(&members);
        let tool = good_tool(&world.tool_dir());
        let found = refusal(world.review(Some(&tool)));
        assert!(check(&found), "{members:?} -> {found:?}");
    }
}

#[test]
fn a_missing_xdelta3_is_a_typed_readiness_refusal() {
    let world = World::new(&[("a.bin.xdelta", VCDIFF)]);
    let missing = world.temp.path().join("no-such-xdelta3");
    assert_eq!(
        refusal(world.review(Some(&missing))),
        DreamcastDeltaRefusal::ToolUnavailable
    );
    // Not executable counts as unavailable too.
    let plain = world.temp.path().join("plain");
    fs::write(&plain, b"x").unwrap();
    assert_eq!(
        refusal(world.review(Some(&plain))),
        DreamcastDeltaRefusal::ToolUnavailable
    );
    // Packages without deltas never need the tool.
    let world = World::new(&[("1ST_READ.BIN", b"new boot")]);
    assert!(world.review(Some(&missing)).is_ok());
}

#[test]
fn staged_output_tampering_is_caught_before_publication() {
    let world = World::new(&[("a.bin.xdelta", VCDIFF)]);
    let tool = good_tool(&world.tool_dir());
    let plan = world.review(Some(&tool)).unwrap();
    let result = tree::prepare(
        &plan.tree,
        |staging| {
            plan.produce(staging)?;
            fs::write(staging.join("a.bin"), b"tampered after decode")
        },
        |staging| plan.verify(staging),
    );
    assert!(result.is_err());
    assert!(!world.destination.exists());
}

#[test]
fn publication_collision_after_delta_staging_never_clobbers_and_undo_is_exact() {
    let world = World::new(&[("a.bin.xdelta", VCDIFF)]);
    let tool = good_tool(&world.tool_dir());
    let prepared = world.review(Some(&tool)).unwrap().prepare().unwrap();
    fs::create_dir(&world.destination).unwrap();
    fs::write(world.destination.join("owner"), b"keep").unwrap();
    assert!(publish(&prepared.journal_path).is_err());
    assert_eq!(fs::read(world.destination.join("owner")).unwrap(), b"keep");
    assert!(!world.destination.join("a.bin").exists());
}
