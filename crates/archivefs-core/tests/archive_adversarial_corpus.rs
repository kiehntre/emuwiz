//! Adversarial regression corpus for 7Z/RAR inspection and extraction
//! (`archivefs_core::archive_workflow`), Hard Job #4.
//!
//! Everything here is synthetic and disposable. The external 7-Zip is
//! replaced by a shell stub that prints a crafted listing and performs a
//! chosen extraction behaviour, so every hostile case runs deterministically
//! without a real hostile archive. A final group uses the real `7z` when
//! one is installed (and is skipped otherwise) to prove the stub models the
//! real tool's argument handling.
//!
//! Every failing case asserts, after the fact, that the destination's
//! parent directory holds exactly what it held before: no stage, no partial
//! output, no stray file.

#![cfg(unix)]

use archivefs_core::archive_workflow::{
    ArchiveEligibility, ArchivePlan, ArchiveTools, RarSupport, extract_archive_with_tools,
    inspect_archive_plan_with_tools,
};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// One disposable world: the archive, the destination parent and the stub.
struct World {
    _temp: tempfile::TempDir,
    archive: PathBuf,
    parent: PathBuf,
    stub_dir: PathBuf,
    stub: PathBuf,
}

impl World {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("library");
        let stub_dir = temp.path().join("tool");
        fs::create_dir_all(&parent).unwrap();
        fs::create_dir_all(&stub_dir).unwrap();
        let archive = temp.path().join("input.7z");
        fs::write(&archive, b"7z\xBC\xAF\x27\x1C not really an archive").unwrap();
        let stub = stub_dir.join("7z");
        fs::write(
            &stub,
            r#"#!/bin/sh
# Stub 7-Zip: `i` prints capabilities, `l` prints the prepared listing,
# `x` performs the prepared behaviour inside the -o directory.
here=$(dirname "$0")
case "$1" in
  i) printf 'Formats:\n 0  7z  7z\n 0  Rar5  rar\n'; exit 0 ;;
  l)
    # Listing runs under a zero RLIMIT_FSIZE (real 7-Zip writes nothing while
    # listing), so this branch must not write files. The summary's Type is
    # the handler the caller pinned with -t, or "none" if it forgot: the
    # parser then refuses, which is how the pinning is verified.
    t=none
    for arg in "$@"; do case "$arg" in -t*) t="${arg#-t}" ;; esac; done
    sed "s/^Type = 7z$/Type = $t/" "$here/listing.txt"; exit 0 ;;
  x)
    out=""
    for arg in "$@"; do
      case "$arg" in -o*) out="${arg#-o}" ;; esac
    done
    [ -n "$out" ] || exit 3
    echo "$out" > "$here/extract-out.txt"
    mode=$(cat "$here/mode")
    case "$mode" in
      ok|partial|oversize|gigantic|extra|missing|wrongsize|symlink|hardlink|nested-escape)
        while IFS='|' read -r path size; do
          [ -n "$path" ] || continue
          case "$path" in */*) mkdir -p "$out/$(dirname "$path")" ;; esac
          head -c "$size" /dev/zero > "$out/$path" || exit 4
        done < "$here/tree.txt"
        ;;
    esac
    case "$mode" in
      ok) exit 0 ;;
      partial) exit 1 ;;
      oversize) head -c 4096 /dev/zero >> "$out/$(head -n1 "$here/tree.txt" | cut -d'|' -f1)"; exit 0 ;;
      gigantic) head -c 3000000 /dev/zero > "$out/$(head -n1 "$here/tree.txt" | cut -d'|' -f1)"; exit 0 ;;
      extra) head -c 3 /dev/zero > "$out/unexpected.bin"; exit 0 ;;
      missing) rm -f "$out/$(head -n1 "$here/tree.txt" | cut -d'|' -f1)"; exit 0 ;;
      wrongsize) head -c 1 /dev/zero >> "$out/$(head -n1 "$here/tree.txt" | cut -d'|' -f1)"; exit 0 ;;
      symlink) ln -s /etc/passwd "$out/link.bin"; exit 0 ;;
      hardlink) ln "$out/$(head -n1 "$here/tree.txt" | cut -d'|' -f1)" "$out/hard.bin"; exit 0 ;;
      nested-escape) exit 0 ;;
      hang) sh -c 'sleep 7331' & sleep 7333; exit 0 ;;
      *) exit 2 ;;
    esac
    ;;
  *) exit 2 ;;
esac
"#,
        )
        .unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
        let world = Self {
            _temp: temp,
            archive,
            parent,
            stub_dir,
            stub,
        };
        world.mode("ok");
        world
    }

    fn tools(&self) -> ArchiveTools {
        // Generous bounds: the stub is a shell script and the host may be
        // heavily loaded. The hang test tightens the extract bound itself.
        ArchiveTools::with_seven_zip(self.stub.clone(), RarSupport::ALL)
            .with_timeouts(Duration::from_secs(60), Duration::from_secs(60))
    }

    fn destination(&self) -> PathBuf {
        self.parent.join("output")
    }

    fn mode(&self, mode: &str) {
        fs::write(self.stub_dir.join("mode"), mode).unwrap();
    }

    /// `members`: (path, size, attributes).
    fn listing(&self, members: &[(&str, &str, &str)]) {
        let mut text = String::from(
            "\n7-Zip 23.01 (x64)\n\nScanning the drive for archives:\n1 file, 30 bytes\n\nListing archive: /proc/self/fd/7\n\n--\nPath = /proc/self/fd/7\nType = 7z\nPhysical Size = 30\n\n----------\n",
        );
        let mut tree = String::new();
        for (path, size, attributes) in members {
            text.push_str(&format!(
                "Path = {path}\nSize = {size}\nPacked Size = 1\nModified = 2024-01-01 00:00:00\nAttributes = {attributes}\nCRC = 00000000\nEncrypted = -\nMethod = LZMA2:12\nBlock = 0\n\n"
            ));
            if !attributes.starts_with('D') {
                tree.push_str(&format!("{path}|{size}\n"));
            }
        }
        fs::write(self.stub_dir.join("listing.txt"), text).unwrap();
        fs::write(self.stub_dir.join("tree.txt"), tree).unwrap();
    }

    /// Raw listing text, for cases the member helper cannot express.
    fn raw_listing(&self, text: &str) {
        fs::write(self.stub_dir.join("listing.txt"), text).unwrap();
        fs::write(self.stub_dir.join("tree.txt"), "").unwrap();
    }

    fn inspect(&self) -> Result<ArchivePlan, String> {
        inspect_archive_plan_with_tools(&self.archive, &self.destination(), &self.tools())
    }

    fn extract(&self, plan: &ArchivePlan) -> Result<(), String> {
        extract_archive_with_tools(plan, &self.tools()).map(|_| ())
    }

    /// Every entry under the destination parent with type, size and inode.
    fn snapshot(&self) -> BTreeMap<String, String> {
        snapshot_tree(&self.parent)
    }

    fn assert_parent_unchanged(&self, before: &BTreeMap<String, String>, context: &str) {
        assert_eq!(
            &self.snapshot(),
            before,
            "{context}: destination parent changed"
        );
    }
}

fn snapshot_tree(root: &Path) -> BTreeMap<String, String> {
    use std::os::unix::fs::MetadataExt;
    let mut entries = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current).unwrap() {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            let kind = if meta.file_type().is_symlink() {
                "symlink"
            } else if meta.is_dir() {
                stack.push(path.clone());
                "dir"
            } else if meta.is_file() {
                "file"
            } else {
                "special"
            };
            entries.insert(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                format!("{kind}:{}:{}", meta.len(), meta.ino()),
            );
        }
    }
    entries
}

fn no_stage_left(world: &World) {
    for entry in fs::read_dir(&world.parent).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        assert!(
            !name.starts_with(".emuwiz-archive-stage-"),
            "staging directory left behind: {name}"
        );
    }
}

fn within_deadline<T: Send + 'static>(
    seconds: u64,
    work: impl FnOnce() -> T + Send + 'static,
) -> T {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    receiver
        .recv_timeout(Duration::from_secs(seconds))
        .expect("operation blocked instead of returning")
}

const FILE: &str = "A -rw-r--r--";
const DIR: &str = "D drwxr-xr-x";
const HEADER: &str = "\n7-Zip 23.01 (x64)\n\n--\nPath = x\nType = 7z\n\n----------\n";

// ---------------------------------------------------------------------
// Inspection: hostile listings never become plans
// ---------------------------------------------------------------------

/// The member names every format must refuse, with the word the explanation
/// must contain so a user learns *why* (never "corrupt" or "missing").
const UNSAFE_NAMES: &[(&str, &str)] = &[
    ("../escape.bin", "`..`"),
    ("/etc/passwd", "absolute"),
    ("a/../../b.bin", "`..`"),
    ("C:/windows/x", "drive"),
    ("..\\x.bin", "backslash"),
    ("Game\\disc.iso", "backslash"),
    ("a/./b.bin", "`.`"),
    ("./x.bin", "`.`"),
    ("a//b.bin", "empty"),
    ("a/", "empty"),
    ("new\nline.bin", "control"),
    ("wild*.bin", "wildcard"),
];

#[test]
fn traversal_and_absolute_member_paths_refuse_sevenz_and_rar_archives() {
    let world = World::new();
    let rar = world.archive.with_extension("rar");
    fs::write(&rar, b"Rar!\x1a\x07\x01\x00 not really").unwrap();
    let before = world.snapshot();
    for (name, why) in UNSAFE_NAMES {
        world.listing(&[("ok.bin", "3", FILE), (name, "3", FILE)]);
        for archive in [&world.archive, &rar] {
            let error =
                inspect_archive_plan_with_tools(archive, &world.destination(), &world.tools())
                    .unwrap_err();
            assert!(
                error.contains("unsafe archive member") || error.contains("MalformedArchive"),
                "{name:?} in {}: {error}",
                archive.display()
            );
            if !name.contains('\n') {
                assert!(
                    error.contains(why),
                    "{name:?}: explanation lacks {why:?}: {error}"
                );
                assert!(
                    !error.to_ascii_lowercase().contains("corrupt")
                        && !error.contains("ToolMissing"),
                    "{name:?}: {error}"
                );
            }
        }
    }
    world.assert_parent_unchanged(&before, "inspection");
}

fn write_zip(path: &Path, members: &[(&str, &[u8])]) {
    use std::io::Write as _;
    let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    for (name, bytes) in members {
        zip.start_file(*name, options).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
}

/// Owner decision 2026-11-06: ZIP names with backslashes (older Windows
/// archivers) are refused, not reinterpreted, and the refusal explains
/// itself. Same rule for every other unsafe shape, in ZIP inspection and in
/// the downloaded mod-package inspector that reuses it.
#[test]
fn unsafe_member_names_refuse_zip_and_mod_package_inspection_with_an_explanation() {
    let temp = tempfile::tempdir().unwrap();
    let good = temp.path().join("good.zip");
    write_zip(&good, &[("Game/disc.iso", b"abc"), ("readme.txt", b"hi")]);
    let plan =
        inspect_archive_plan_with_tools(&good, &temp.path().join("out"), &ArchiveTools::none())
            .unwrap();
    assert_eq!(plan.entries.len(), 2);
    assert!(plan.source_identity.is_some());
    for (index, (name, why)) in UNSAFE_NAMES.iter().enumerate() {
        if name.ends_with('/') {
            // The zip crate writes a trailing slash as a directory entry.
            continue;
        }
        let archive = temp.path().join(format!("bad-{index}.zip"));
        write_zip(&archive, &[("ok.bin", b"abc"), (name, b"abc")]);
        let error = inspect_archive_plan_with_tools(
            &archive,
            &temp.path().join("out"),
            &ArchiveTools::none(),
        )
        .unwrap_err();
        assert!(error.contains("unsafe archive member"), "{name:?}: {error}");
        assert!(
            error.contains(why),
            "{name:?}: explanation lacks {why:?}: {error}"
        );
        assert!(
            !error.to_ascii_lowercase().contains("corrupt"),
            "{name:?}: {error}"
        );
        let mod_error =
            archivefs_core::archived_mod_package::inspect_archived_mod_package(&archive)
                .unwrap_err();
        assert!(
            mod_error.contains("unsafe archive member"),
            "{name:?}: {mod_error}"
        );
        assert!(mod_error.contains(why), "{name:?}: {mod_error}");
    }
    assert!(!temp.path().join("out").exists());
}

#[test]
fn symlink_hardlink_device_and_fifo_members_refuse_the_archive() {
    let world = World::new();
    for attributes in [
        "A lrwxrwxrwx",
        "A brw-rw----",
        "A crw-rw-rw-",
        "A prw-------",
        "A srwxrwxrwx",
    ] {
        world.listing(&[("ok.bin", "3", FILE), ("special", "3", attributes)]);
        let error = world.inspect().unwrap_err();
        assert!(
            error.contains("UnsupportedMemberType"),
            "{attributes}: {error}"
        );
    }
    world.raw_listing(&format!(
        "{HEADER}Path = l\nSize = 3\nAttributes = A -rw-r--r--\nSymbolic Link = ../../outside\n\n"
    ));
    assert!(
        world
            .inspect()
            .unwrap_err()
            .contains("UnsupportedMemberType")
    );
    world.raw_listing(&format!(
        "{HEADER}Path = h\nSize = 3\nAttributes = A -rw-r--r--\nHard Link = ok.bin\n\n"
    ));
    assert!(
        world
            .inspect()
            .unwrap_err()
            .contains("UnsupportedMemberType")
    );
}

#[test]
fn names_with_newlines_control_characters_and_wildcards_refuse_the_archive() {
    let world = World::new();
    // Embedded newline that forges a hidden record.
    world.raw_listing(&format!(
        "{HEADER}Path = shown.bin\nSize = 3\nAttributes = A -rw-r--r--\n\nPath = hidden\n../../x\nSize = 3\nAttributes = A -rw-r--r--\n\n"
    ));
    assert!(world.inspect().unwrap_err().contains("MalformedArchive"));
    // Embedded newline that duplicates a key.
    world.raw_listing(&format!(
        "{HEADER}Path = a.bin\nSize = 3\nSize = 9999999999\nAttributes = A -rw-r--r--\n\n"
    ));
    assert!(world.inspect().unwrap_err().contains("MalformedArchive"));
    for name in [
        "tab\there.bin",
        "esc\x1b[0m.bin",
        "wild*.bin",
        "one?.bin",
        "-switch.bin",
        "cr\r.bin",
    ] {
        world.listing(&[(name, "3", FILE)]);
        let error = world.inspect().unwrap_err();
        assert!(
            error.contains("UnsafeMember") || error.contains("MalformedArchive"),
            "{name:?}: {error}"
        );
    }
}

#[test]
fn duplicate_and_case_colliding_member_names_refuse_the_archive() {
    let world = World::new();
    world.listing(&[("Game/A.bin", "3", FILE), ("game/a.bin", "3", FILE)]);
    assert!(
        world
            .inspect()
            .unwrap_err()
            .contains("case-folding member collision")
    );
    world.listing(&[("same.bin", "3", FILE), ("same.bin", "4", FILE)]);
    assert!(
        world
            .inspect()
            .unwrap_err()
            .contains("case-folding member collision")
    );
}

#[test]
fn bombs_and_excessive_member_counts_refuse_the_archive() {
    let world = World::new();
    // 30-byte archive claiming 64 GiB of output: ratio and total both fail.
    world.listing(&[("huge.bin", "68719476736", FILE)]);
    assert!(
        world
            .inspect()
            .unwrap_err()
            .contains("expansion exceeds safety policy")
    );
    // Modest ratio but past the member-count bound.
    let mut members = Vec::new();
    let names: Vec<String> = (0..=100_000).map(|index| format!("m{index}")).collect();
    for name in &names {
        members.push((name.as_str(), "0", FILE));
    }
    world.listing(&members);
    assert!(world.inspect().unwrap_err().contains("TooManyEntries"));
}

#[test]
fn truncated_and_malformed_listings_refuse_the_archive() {
    let world = World::new();
    world.raw_listing("");
    assert!(world.inspect().unwrap_err().contains("MalformedArchive"));
    world.raw_listing("7-Zip 23.01\n\nERROR: Can not open the file as archive\n");
    assert!(world.inspect().unwrap_err().contains("MalformedArchive"));
    world.raw_listing("Type = 7z\n\n----------\nPath = a.bin\nSize = 3\n");
    assert!(world.inspect().unwrap_err().contains("MalformedArchive"));
    world.raw_listing(
        "Type = 7z\n\n----------\nPath = a.bin\nSize = three\nAttributes = A -rw-r--r--\n",
    );
    assert!(world.inspect().unwrap_err().contains("MalformedArchive"));
    world.raw_listing(
        "Type = 7z\n\n----------\nPath = a.bin\nSize = 3\nAttributes = A -rw-r--r--\nencrypted?\n",
    );
    assert!(world.inspect().unwrap_err().contains("MalformedArchive"));
    // A listing whose summary names another handler (7-Zip auto-detected a
    // different container) is refused as unsupported. (The stub prints the
    // -t handler it was given as the summary Type, so every passing case in
    // this file also proves the tool was pinned to the signature's handler.)
    world.raw_listing(
        "Type = Iso\n\n----------\nPath = a.bin\nSize = 3\nAttributes = A -rw-r--r--\n\n",
    );
    assert!(world.inspect().unwrap_err().contains("Unsupported"));
    // A ZIP under a .7z name is a ZIP (signature wins) and never reaches the
    // external tool: the error comes from the ZIP inspector, not from the
    // external listing path.
    fs::write(&world.archive, b"PK\x03\x04 renamed zip").unwrap();
    let error = world.inspect().unwrap_err();
    assert!(
        !error.contains("archive inspection failed"),
        "renamed ZIP reached the external tool path: {error}"
    );
}

#[test]
fn a_fifo_or_directory_in_the_archives_place_is_refused_promptly() {
    let world = World::new();
    world.listing(&[("a.bin", "3", FILE)]);
    fs::remove_file(&world.archive).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&world.archive)
            .status()
            .unwrap()
            .success()
    );
    let archive = world.archive.clone();
    let destination = world.destination();
    let tools = world.tools();
    let error = within_deadline(5, move || {
        inspect_archive_plan_with_tools(&archive, &destination, &tools).unwrap_err()
    });
    assert!(
        error.contains("not regular") || error.contains("refused"),
        "{error}"
    );
    let _ = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&world.archive);
}

// ---------------------------------------------------------------------
// Extraction: the plan is bound, the stage is contained, failures clean up
// ---------------------------------------------------------------------

#[test]
fn a_successful_extraction_publishes_exactly_the_planned_tree_and_nothing_else() {
    let world = World::new();
    world.listing(&[
        ("game", "0", DIR),
        ("game/disc.iso", "5", FILE),
        ("readme.txt", "2", FILE),
    ]);
    let plan = world.inspect().unwrap();
    assert_eq!(plan.eligibility, ArchiveEligibility::Ready);
    assert!(plan.source_identity.is_some());
    assert_eq!(plan.expanded_bytes, 7);
    let before = world.snapshot();
    world.extract(&plan).unwrap();
    let after = world.snapshot();
    let new_entries: Vec<&String> = after
        .keys()
        .filter(|key| !before.contains_key(*key))
        .collect();
    assert_eq!(
        new_entries,
        vec![
            "output",
            "output/game",
            "output/game/disc.iso",
            "output/readme.txt"
        ]
    );
    assert_eq!(
        fs::metadata(world.destination().join("game/disc.iso"))
            .unwrap()
            .len(),
        5
    );
    no_stage_left(&world);
    // The tool was given the stage through a pinned descriptor, never a path
    // it could have re-resolved.
    let out = fs::read_to_string(world.stub_dir.join("extract-out.txt")).unwrap();
    assert!(out.trim().starts_with("/proc/self/fd/"), "{out}");
    // A second extraction into the same destination is refused before any
    // work, because the destination exists.
    let before = world.snapshot();
    let error = world.extract(&plan).unwrap_err();
    assert!(error.contains("already exists"), "{error}");
    world.assert_parent_unchanged(&before, "existing destination");
}

#[test]
fn an_archive_changed_after_preview_is_refused_at_extraction() {
    let world = World::new();
    world.listing(&[("a.bin", "3", FILE)]);
    let plan = world.inspect().unwrap();
    // Same path, same inode, new bytes (in-place rewrite): size and mtime differ.
    fs::write(
        &world.archive,
        b"7z\xBC\xAF\x27\x1C replaced after the preview was approved",
    )
    .unwrap();
    let before = world.snapshot();
    let error = world.extract(&plan).unwrap_err();
    assert!(error.contains("SourceChanged"), "{error}");
    world.assert_parent_unchanged(&before, "changed archive");
    no_stage_left(&world);
    // Same bytes, different inode (replace-by-rename) is also a different archive.
    let plan = world.inspect().unwrap();
    let swap = world.archive.with_extension("swap");
    fs::copy(&world.archive, &swap).unwrap();
    fs::rename(&swap, &world.archive).unwrap();
    let error = world.extract(&plan).unwrap_err();
    assert!(error.contains("SourceChanged"), "{error}");
    // An unbound plan is refused outright.
    let mut unbound = world.inspect().unwrap();
    unbound.source_identity = None;
    assert!(world.extract(&unbound).unwrap_err().contains("not bound"));
}

#[test]
fn a_tool_that_writes_something_other_than_the_plan_is_refused_and_cleaned_up() {
    let world = World::new();
    world.listing(&[("a.bin", "3", FILE), ("b.bin", "4", FILE)]);
    let plan = world.inspect().unwrap();
    let before = world.snapshot();
    // `gigantic` writes 3 MB into a 3-byte member: the RLIMIT_FSIZE cap (1 MiB
    // floor) stops the tool mid-write, and verification refuses the rest.
    for mode in [
        "extra",
        "missing",
        "wrongsize",
        "oversize",
        "gigantic",
        "symlink",
        "hardlink",
    ] {
        world.mode(mode);
        let error = world.extract(&plan).unwrap_err();
        assert!(
            error.contains("verification failed")
                || error.contains("detected")
                || error.contains("extraction failed"),
            "{mode}: {error}"
        );
        world.assert_parent_unchanged(&before, mode);
        no_stage_left(&world);
    }
}

#[test]
fn an_extraction_interrupted_mid_operation_leaves_nothing_behind() {
    let world = World::new();
    world.listing(&[("a.bin", "3", FILE), ("b.bin", "4", FILE)]);
    let plan = world.inspect().unwrap();
    let before = world.snapshot();
    world.mode("partial");
    let error = world.extract(&plan).unwrap_err();
    assert!(error.contains("extraction failed"), "{error}");
    world.assert_parent_unchanged(&before, "partial extraction");
    no_stage_left(&world);
}

#[test]
fn a_hanging_tool_is_killed_with_its_descendants_and_the_stage_is_removed() {
    let world = World::new();
    world.listing(&[("a.bin", "3", FILE)]);
    let plan = world.inspect().unwrap();
    let before = world.snapshot();
    world.mode("hang");
    let started = std::time::Instant::now();
    let tight = world
        .tools()
        .with_timeouts(Duration::from_secs(60), Duration::from_secs(3));
    let error = extract_archive_with_tools(&plan, &tight)
        .map(|_| ())
        .unwrap_err();
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "timeout did not fire"
    );
    assert!(error.contains("ToolLimitExceeded"), "{error}");
    world.assert_parent_unchanged(&before, "hanging tool");
    no_stage_left(&world);
    // The descendant `sleep 7331` (grandchild) and the direct `sleep 7333`
    // were in the tool's process group and must be gone.
    std::thread::sleep(Duration::from_millis(200));
    let survivors = std::process::Command::new("pgrep")
        .args(["-f", "sleep 733[13]"])
        .output()
        .unwrap();
    assert!(
        survivors.stdout.is_empty(),
        "descendants survived the timeout: {}",
        String::from_utf8_lossy(&survivors.stdout)
    );
}

#[test]
fn planned_expansion_without_room_is_refused_before_any_write() {
    let world = World::new();
    world.listing(&[("a.bin", "3", FILE)]);
    let mut plan = world.inspect().unwrap();
    plan.expanded_bytes = u64::MAX / 4;
    let before = world.snapshot();
    let error = world.extract(&plan).unwrap_err();
    assert!(error.contains("InsufficientSpace"), "{error}");
    world.assert_parent_unchanged(&before, "insufficient space");
    no_stage_left(&world);
}

#[test]
fn unsafe_destinations_are_refused_before_any_write() {
    let world = World::new();
    world.listing(&[("a.bin", "3", FILE)]);
    let plan = world.inspect().unwrap();
    let before = world.snapshot();
    for destination in [
        PathBuf::from("relative/output"),
        world.parent.join("../output"),
        world.parent.join("missing/output"),
        world.parent.clone(),
    ] {
        let mut unsafe_plan = plan.clone();
        unsafe_plan.destination = destination.clone();
        assert!(
            world.extract(&unsafe_plan).is_err(),
            "{}",
            destination.display()
        );
    }
    // A destination whose parent is a symlink to elsewhere: the published
    // tree lands in the symlink's real target directory (that is where the
    // user pointed), never anywhere else, and the symlink itself is untouched.
    let elsewhere = world.parent.join("elsewhere-real");
    fs::create_dir(&elsewhere).unwrap();
    let link = world.parent.join("linked");
    std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
    let mut linked_plan = plan.clone();
    linked_plan.destination = link.join("output");
    world.extract(&linked_plan).unwrap();
    assert!(elsewhere.join("output/a.bin").is_file());
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    fs::remove_dir_all(&elsewhere).unwrap();
    fs::remove_file(&link).unwrap();
    world.assert_parent_unchanged(&before, "destination checks");
}

#[test]
fn a_destination_created_during_extraction_is_not_replaced() {
    // The stub `ok` mode extracts fine; the destination appears between the
    // pre-check and the publish. RENAME_NOREPLACE must refuse and the stage
    // must be removed.
    let world = World::new();
    world.listing(&[("a.bin", "3", FILE)]);
    let plan = world.inspect().unwrap();
    // Make the stub create the destination itself as a side effect.
    let stub = fs::read_to_string(&world.stub).unwrap().replace(
        "      ok) exit 0 ;;",
        &format!(
            "      ok) mkdir -p '{}'; echo racer > '{}/racer.txt'; exit 0 ;;",
            world.destination().display(),
            world.destination().display()
        ),
    );
    fs::write(&world.stub, stub).unwrap();
    let error = world.extract(&plan).unwrap_err();
    assert!(error.contains("could not publish"), "{error}");
    assert_eq!(
        fs::read_to_string(world.destination().join("racer.txt")).unwrap(),
        "racer\n"
    );
    assert!(!world.destination().join("a.bin").exists());
    no_stage_left(&world);
}

// ---------------------------------------------------------------------
// Real 7-Zip, when installed: the stub models its argument handling
// ---------------------------------------------------------------------

fn real_tools() -> Option<ArchiveTools> {
    let tools = ArchiveTools::discover();
    tools.seven_zip().is_some().then_some(tools)
}

#[test]
fn real_sevenz_round_trip_and_change_detection() {
    let Some(tools) = real_tools() else {
        eprintln!("skipped: no 7-Zip installed");
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let source_tree = temp.path().join("tree");
    fs::create_dir_all(source_tree.join("Game/Nested")).unwrap();
    fs::write(source_tree.join("Game/disc.iso"), b"12345").unwrap();
    fs::write(source_tree.join("Game/Nested/ünïcödé.bin"), b"abc").unwrap();
    fs::write(source_tree.join("readme.txt"), b"hi").unwrap();
    let archive = temp.path().join("real.7z");
    let status = std::process::Command::new(tools.seven_zip().unwrap())
        .args(["a", "-bd", "-bb0", "-y"])
        .arg(&archive)
        .arg(".")
        .current_dir(&source_tree)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    let parent = temp.path().join("library");
    fs::create_dir(&parent).unwrap();
    let destination = parent.join("out");
    let plan = inspect_archive_plan_with_tools(&archive, &destination, &tools).unwrap();
    let paths: Vec<&str> = plan
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    assert!(paths.contains(&"Game/disc.iso"), "{paths:?}");
    assert!(paths.contains(&"Game/Nested/ünïcödé.bin"), "{paths:?}");
    assert_eq!(plan.expanded_bytes, 10);
    extract_archive_with_tools(&plan, &tools).unwrap();
    assert_eq!(
        fs::read(destination.join("Game/disc.iso")).unwrap(),
        b"12345"
    );
    assert_eq!(
        fs::read(destination.join("Game/Nested/ünïcödé.bin")).unwrap(),
        b"abc"
    );
    assert_eq!(
        snapshot_tree(&parent).len(),
        6,
        "{:?}",
        snapshot_tree(&parent)
    );
    // Appending one byte after inspection is detected.
    let plan = inspect_archive_plan_with_tools(&archive, &parent.join("again"), &tools).unwrap();
    let mut bytes = fs::read(&archive).unwrap();
    bytes.push(0);
    fs::write(&archive, bytes).unwrap();
    let error = extract_archive_with_tools(&plan, &tools).unwrap_err();
    assert!(error.contains("SourceChanged"), "{error}");
    assert!(!parent.join("again").exists());
}

#[test]
fn real_sevenz_refuses_symlink_members_and_full_path_members() {
    let Some(tools) = real_tools() else {
        eprintln!("skipped: no 7-Zip installed");
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let source_tree = temp.path().join("tree");
    fs::create_dir_all(&source_tree).unwrap();
    fs::write(source_tree.join("real.bin"), b"abc").unwrap();
    std::os::unix::fs::symlink("/etc/passwd", source_tree.join("link.bin")).unwrap();
    let archive = temp.path().join("links.7z");
    // `-snl` stores the symlink as a link.
    let status = std::process::Command::new(tools.seven_zip().unwrap())
        .args(["a", "-bd", "-bb0", "-y", "-snl"])
        .arg(&archive)
        .arg(".")
        .current_dir(&source_tree)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    let destination = temp.path().join("out");
    let error = inspect_archive_plan_with_tools(&archive, &destination, &tools).unwrap_err();
    assert!(error.contains("UnsupportedMemberType"), "{error}");
    assert!(!destination.exists());
}

#[test]
fn a_sevenz_without_rar_codecs_is_reported_as_incapable_not_missing() {
    let world = World::new();
    world.listing(&[("a.bin", "3", FILE)]);
    let rar = world.archive.with_extension("rar");
    fs::write(&rar, b"Rar!\x1a\x07\x01\x00 not really").unwrap();
    let incapable = ArchiveTools::with_seven_zip(world.stub.clone(), RarSupport::NONE);
    let error =
        inspect_archive_plan_with_tools(&rar, &world.destination(), &incapable).unwrap_err();
    assert!(error.contains("ToolLacksFormatSupport"), "{error}");
    let error = inspect_archive_plan_with_tools(&rar, &world.destination(), &ArchiveTools::none())
        .unwrap_err();
    assert!(error.contains("ToolMissing"), "{error}");
    // With RAR codecs the same stub listing becomes a plan.
    let plan = inspect_archive_plan_with_tools(&rar, &world.destination(), &world.tools()).unwrap();
    assert_eq!(plan.entries.len(), 1);
    // Multi-volume names are refused before any tool runs.
    for name in ["set.part1.rar", "set.part02.rar", "set.r00", "set.r01"] {
        let volume = world.archive.with_file_name(name);
        fs::write(&volume, b"Rar!\x1a\x07\x01\x00").unwrap();
        let error = inspect_archive_plan_with_tools(&volume, &world.destination(), &world.tools())
            .unwrap_err();
        assert!(error.contains("MultipartIncomplete"), "{name}: {error}");
    }
}
