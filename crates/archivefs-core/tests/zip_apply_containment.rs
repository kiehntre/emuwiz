//! Preservation regressions for the temporary ZIP Apply gate.
use archivefs_core::zip_converter::{
    ZipError, compress_verified, extract_verified, preview_compress, preview_extract,
};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

fn write_zip(path: &Path) {
    let mut writer = zip::ZipWriter::new(fs::File::create(path).unwrap());
    writer
        .start_file("nested/game.bin", zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"synthetic game").unwrap();
    writer.finish().unwrap();
}

#[derive(Debug, PartialEq, Eq)]
struct Entry {
    contents: Vec<u8>,
    directory: bool,
    link: Option<PathBuf>,
    #[cfg(unix)]
    identity: (u64, u64, u32, i64, i64, i64, i64),
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Entry> {
    fn visit(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, Entry>) {
        let meta = fs::symlink_metadata(path).unwrap();
        let link = meta.file_type().is_symlink();
        entries.insert(
            path.strip_prefix(root).unwrap().to_owned(),
            Entry {
                contents: if meta.is_file() && !link {
                    fs::read(path).unwrap()
                } else {
                    Vec::new()
                },
                directory: meta.is_dir(),
                link: link.then(|| fs::read_link(path).unwrap()),
                #[cfg(unix)]
                identity: {
                    use std::os::unix::fs::MetadataExt;
                    (
                        meta.dev(),
                        meta.ino(),
                        meta.mode(),
                        meta.mtime(),
                        meta.mtime_nsec(),
                        meta.ctime(),
                        meta.ctime_nsec(),
                    )
                },
            },
        );
        if meta.is_dir() && !link {
            for entry in fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), entries);
            }
        }
    }
    let mut entries = BTreeMap::new();
    visit(root, root, &mut entries);
    entries
}

// Observe even transient staging creation/deletion that a final snapshot misses.
// Reads and access-time updates are deliberately outside this mutation mask.
#[cfg(target_os = "linux")]
struct MutationWatch(fs::File);

#[cfg(target_os = "linux")]
impl MutationWatch {
    fn new(root: &Path, entries: &BTreeMap<PathBuf, Entry>) -> Self {
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::ffi::OsStrExt;
        // SAFETY: no pointer arguments; the new descriptor is owned exactly once.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        assert!(fd >= 0, "{}", std::io::Error::last_os_error());
        // SAFETY: successful inotify_init1 returned a fresh owned descriptor.
        let watch = Self(unsafe { fs::File::from_raw_fd(fd) });
        let mask = libc::IN_CREATE
            | libc::IN_DELETE
            | libc::IN_MOVED_FROM
            | libc::IN_MOVED_TO
            | libc::IN_MODIFY
            | libc::IN_ATTRIB
            | libc::IN_CLOSE_WRITE
            | libc::IN_DELETE_SELF
            | libc::IN_MOVE_SELF;
        for (relative, entry) in entries {
            if !entry.directory {
                continue;
            }
            let path = std::ffi::CString::new(root.join(relative).as_os_str().as_bytes()).unwrap();
            // SAFETY: live descriptor and valid NUL-terminated path throughout call.
            let wd = unsafe { libc::inotify_add_watch(watch.0.as_raw_fd(), path.as_ptr(), mask) };
            assert!(wd >= 0, "{}", std::io::Error::last_os_error());
        }
        watch
    }
    fn assert_quiet(&mut self) {
        use std::io::Read;
        let mut events = [0_u8; 4096];
        let error = self
            .0
            .read(&mut events)
            .expect_err("Apply emitted filesystem mutation events");
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    }
}

fn assert_refusal_matrix<T: std::fmt::Debug + PartialEq>(
    apply: impl Fn(&Path, &Path) -> Result<T, ZipError>,
) {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.bin");
    let archive = root.path().join("source.zip");
    let folder = root.path().join("source-folder");
    let invalid = root.path().join("invalid.zip");
    let occupied = root.path().join("occupied");
    let occupied_dir = root.path().join("occupied-dir");
    fs::write(&source, b"synthetic source").unwrap();
    write_zip(&archive);
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("game.bin"), b"folder source").unwrap();
    fs::write(&invalid, b"invalid archive").unwrap();
    fs::write(&occupied, b"existing destination").unwrap();
    fs::create_dir(&occupied_dir).unwrap();
    fs::write(
        occupied_dir.join("keep.bin"),
        b"existing destination member",
    )
    .unwrap();

    // Simulate abandoned staging names now occupied by unrelated replacements.
    let replaced_file = root.path().join(".emuwiz-zip-substituted.tmp");
    fs::write(&replaced_file, b"old staging").unwrap();
    fs::rename(&replaced_file, root.path().join("displaced-stage")).unwrap();
    fs::write(&replaced_file, b"unowned replacement file").unwrap();
    let replaced_dir = root.path().join(".emuwiz-zip-extract-substituted");
    fs::create_dir(&replaced_dir).unwrap();
    fs::rename(&replaced_dir, root.path().join("displaced-tree")).unwrap();
    fs::create_dir(&replaced_dir).unwrap();
    fs::write(replaced_dir.join("keep.bin"), b"unowned replacement tree").unwrap();
    fs::write(
        root.path().join(".emuwiz-zip-probe-substituted.tmp"),
        b"unowned probe replacement",
    )
    .unwrap();

    let legacy_stage = root
        .path()
        .join(format!(".emuwiz-archive-stage-{}", std::process::id()));
    fs::create_dir(&legacy_stage).unwrap();
    fs::write(legacy_stage.join("keep.bin"), b"unowned legacy tree").unwrap();
    fs::write(
        root.path()
            .join(format!(".emuwiz-archive-stage-{}.zip", std::process::id())),
        b"unowned legacy file",
    )
    .unwrap();

    let mut sources = vec![
        source,
        archive,
        folder,
        invalid,
        root.path().join("missing-source"),
    ];
    let mut destinations = vec![
        root.path().join("new-output"),
        occupied,
        occupied_dir,
        root.path().join("missing-parent/output"),
        replaced_file,
        replaced_dir,
    ];
    #[cfg(unix)]
    {
        let source_link = root.path().join("source-link");
        let destination_link = root.path().join("destination-link");
        let dangling = root.path().join("dangling-destination");
        std::os::unix::fs::symlink(&sources[0], &source_link).unwrap();
        std::os::unix::fs::symlink(&destinations[1], &destination_link).unwrap();
        std::os::unix::fs::symlink("missing-target", &dangling).unwrap();
        sources.push(source_link);
        destinations.extend([destination_link, dangling]);
    }
    let before = snapshot(root.path());
    #[cfg(target_os = "linux")]
    let mut watch = MutationWatch::new(root.path(), &before);
    for source in &sources {
        for destination in &destinations {
            assert_eq!(
                apply(source, destination),
                Err(ZipError::ApplyUnavailable),
                "source={source:?}, destination={destination:?}"
            );
        }
    }
    assert_eq!(snapshot(root.path()), before);
    #[cfg(target_os = "linux")]
    watch.assert_quiet();
}

#[test]
fn compression_apply_always_refuses_without_mutation_or_cleanup() {
    assert_refusal_matrix(compress_verified);
}

#[test]
fn extraction_apply_always_refuses_without_mutation_or_cleanup() {
    assert_refusal_matrix(extract_verified);
}

#[test]
fn valid_previews_inspect_members_without_mutation() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.bin");
    let archive = root.path().join("source.zip");
    fs::write(&source, b"synthetic source").unwrap();
    write_zip(&archive);
    let before = snapshot(root.path());
    #[cfg(target_os = "linux")]
    let mut watch = MutationWatch::new(root.path(), &before);
    let compress = preview_compress(&source, &root.path().join("new.zip")).unwrap();
    assert_eq!(compress.entries[0].name, "source.bin");
    assert_eq!(compress.total_size, 16);
    let extract = preview_extract(&archive, &root.path().join("output")).unwrap();
    assert_eq!(extract.entries[0].name, "nested/game.bin");
    assert_eq!(extract.total_size, 14);
    assert_eq!(snapshot(root.path()), before);
    #[cfg(target_os = "linux")]
    watch.assert_quiet();
}

#[test]
fn refusal_message_explains_preservation_and_read_only_preview() {
    let message = ZipError::ApplyUnavailable.to_string();
    assert!(message.contains("temporarily unavailable"));
    assert!(message.contains("preservation safety"));
    assert!(message.contains("staging ownership"));
    assert!(message.contains("No output was created or extracted"));
    assert!(message.contains("Read-only preview remains available"));
}

fn legacy_error(error: String) -> ZipError {
    assert_eq!(error, ZipError::ApplyUnavailable.to_string());
    ZipError::ApplyUnavailable
}

fn zip_plan(source: &Path, destination: &Path) -> archivefs_core::archive_workflow::ArchivePlan {
    use archivefs_core::archive_workflow::{
        ArchiveEligibility, ArchiveFormat, ArchiveOperation, ArchivePlan,
    };
    ArchivePlan {
        operation: ArchiveOperation::ExtractArchive,
        format: ArchiveFormat::Zip,
        source: source.to_owned(),
        destination: destination.to_owned(),
        entries: Vec::new(),
        compressed_bytes: None,
        expanded_bytes: 0,
        eligibility: ArchiveEligibility::Ready,
        warnings: Vec::new(),
    }
}

#[test]
fn legacy_zip_creation_refuses_before_publication() {
    assert_refusal_matrix(|source, destination| {
        archivefs_core::archive_workflow::create_zip(source, destination).map_err(legacy_error)
    });
}

#[test]
fn legacy_zip_extraction_refuses_before_publication() {
    assert_refusal_matrix(|source, destination| {
        archivefs_core::archive_workflow::extract_zip(&zip_plan(source, destination))
            .map_err(legacy_error)
    });
}

#[test]
fn archive_dispatch_zip_branch_refuses_before_publication() {
    assert_refusal_matrix(|source, destination| {
        archivefs_core::archive_workflow::extract_archive(&zip_plan(source, destination))
            .map_err(legacy_error)
    });
}

#[test]
fn archive_plan_inspection_remains_read_only_and_operational() {
    let root = tempfile::tempdir().unwrap();
    let archive = root.path().join("source.zip");
    write_zip(&archive);
    let before = snapshot(root.path());
    #[cfg(target_os = "linux")]
    let mut watch = MutationWatch::new(root.path(), &before);
    let plan = archivefs_core::archive_workflow::inspect_archive_plan(
        &archive,
        &root.path().join("output"),
    )
    .unwrap();
    assert_eq!(plan.entries[0].path, "nested/game.bin");
    assert_eq!(snapshot(root.path()), before);
    #[cfg(target_os = "linux")]
    watch.assert_quiet();
}
