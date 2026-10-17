#![cfg(target_os = "linux")]
use archivefs_core::zip_converter::reviewed_apply::*;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
fn no() -> AtomicBool {
    AtomicBool::new(false)
}
fn zip(path: &Path, entries: &[(&str, &[u8])]) {
    let mut writer = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, bytes) in entries {
        writer
            .start_file(
                *name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
}
fn fixture() -> (tempfile::TempDir, ZipPreview) {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.zip");
    zip(&source, &[("game.bin", b"approved A")]);
    let p = preview_extract(&source, &root.path().join("output.bin")).unwrap();
    (root, p)
}
#[test]
fn reviewed_compression_and_single_file_extraction_preserve_originals() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("game.bin");
    fs::write(&source, b"reviewed bytes").unwrap();
    let archive = root.path().join("game.zip");
    let p = preview_compress(&source, &archive).unwrap();
    assert!(p.can_apply());
    apply_reviewed(&p, &no()).unwrap();
    let bytes = fs::read(&archive).unwrap();
    let p = preview_extract(&archive, &root.path().join("copy.bin")).unwrap();
    assert_eq!(p.archive_sha256().unwrap().len(), 64);
    apply_reviewed(&p, &no()).unwrap();
    assert_eq!(fs::read(source).unwrap(), b"reviewed bytes");
    assert_eq!(
        fs::read(root.path().join("copy.bin")).unwrap(),
        b"reviewed bytes"
    );
    assert_eq!(fs::read(archive).unwrap(), bytes);
}
#[test]
fn folder_compression_preserves_empty_dirs_and_content() {
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("input");
    fs::create_dir_all(src.join("empty")).unwrap();
    fs::write(src.join("game"), b"A").unwrap();
    let p = preview_compress(&src, &root.path().join("out.zip")).unwrap();
    apply_reviewed(&p, &no()).unwrap();
    let mut z = zip::ZipArchive::new(fs::File::open(&p.destination).unwrap()).unwrap();
    assert_eq!(z.len(), 3);
    assert!(z.by_name("input/empty/").unwrap().is_dir());
}
#[test]
fn stale_display_source_archive_bytes_and_inode_refuse_before_publication() {
    for change in 0..5 {
        let (root, mut p) = fixture();
        let expected = p.destination.clone();
        match change {
            0 => p.destination = root.path().join("other"),
            1 => p.entries[0].name = "other".into(),
            2 => zip(&p.source, &[("game.bin", b"differentB")]),
            3 => {
                let replacement = root.path().join("replacement");
                fs::copy(&p.source, &replacement).unwrap();
                fs::rename(replacement, &p.source).unwrap();
            }
            _ => {
                fs::remove_file(&p.source).unwrap();
            }
        }
        assert!(matches!(
            apply_reviewed(&p, &no()),
            Err(ZipError::StalePreview(_))
        ));
        assert!(!expected.exists());
    }
}
#[test]
fn substitution_at_every_precommit_boundary_is_refused() {
    for boundary in [
        StagePhase::EntryStaged,
        StagePhase::BeforePublication,
        StagePhase::PublishReady,
    ] {
        let (_root, p) = fixture();
        let result = apply_with_observer(&p, &no(), |phase| {
            if phase == boundary {
                zip(&p.source, &[("game.bin", b"foreign B!")]);
            }
            Ok(())
        });
        assert!(matches!(result, Err(ZipError::StalePreview(_))));
        assert!(!p.destination.exists());
    }
}
#[test]
fn destination_collisions_and_dangling_symlinks_never_overwrite() {
    use std::os::unix::fs::symlink;
    for link in [false, true] {
        let (_root, p) = fixture();
        let r = apply_with_observer(&p, &no(), |phase| {
            if phase == StagePhase::PublishReady {
                if link {
                    symlink("absent", &p.destination).unwrap();
                } else {
                    fs::write(&p.destination, b"foreign B").unwrap();
                }
            }
            Ok(())
        });
        assert!(r.is_err());
        if link {
            assert_eq!(
                fs::read_link(&p.destination).unwrap(),
                PathBuf::from("absent")
            );
        } else {
            assert_eq!(fs::read(&p.destination).unwrap(), b"foreign B");
        }
    }
}
#[test]
fn named_staging_lookalikes_are_never_published_or_cleaned() {
    let (root, p) = fixture();
    let foreign = root.path().join(".emuwiz-zip-foreign.tmp");
    fs::write(&foreign, b"B").unwrap();
    let r = apply_with_observer(&p, &no(), |phase| {
        if phase == StagePhase::BeforePublication {
            fs::create_dir(root.path().join(".emuwiz-zip-extract-foreign")).unwrap();
        }
        Ok(())
    });
    r.unwrap();
    assert_eq!(fs::read(&p.destination).unwrap(), b"approved A");
    assert_eq!(fs::read(foreign).unwrap(), b"B");
    assert!(root.path().join(".emuwiz-zip-extract-foreign").is_dir());
}
#[test]
fn unsupported_tree_layouts_remain_preview_only_and_create_nothing() {
    for entries in [
        vec![("a", &b"a"[..]), ("b", &b"b"[..])],
        vec![("dir/a", &b"a"[..])],
    ] {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("s.zip");
        zip(&source, &entries);
        let p = preview_extract(&source, &root.path().join("out")).unwrap();
        assert!(!p.can_apply());
        assert_eq!(apply_reviewed(&p, &no()), Err(ZipError::UnsupportedLayout));
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}
#[test]
fn traversal_duplicate_alias_and_conflicting_members_refuse() {
    for names in [
        vec!["../bad"],
        vec!["/bad"],
        vec!["a\\b"],
        vec!["a/./b"],
        vec!["a//b"],
        vec!["A", "a"],
        vec!["a", "a/b"],
    ] {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("s.zip");
        let entries: Vec<_> = names.iter().map(|n| (*n, &b"x"[..])).collect();
        zip(&source, &entries);
        assert!(preview_extract(&source, &root.path().join("out")).is_err());
    }
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("s.zip");
    zip(&source, &[("one", b"x"), ("two", b"y")]);
    let bytes = fs::read(&source).unwrap();
    let mut modified = bytes.clone();
    for i in 0..bytes.len() - 2 {
        if &bytes[i..i + 3] == b"two" {
            modified[i..i + 3].copy_from_slice(b"one");
        }
    }
    fs::write(&source, modified).unwrap();
    assert_eq!(
        preview_extract(&source, &root.path().join("out")).unwrap_err(),
        ZipError::DuplicatePath
    );
}
#[test]
fn symlinks_in_source_and_destination_ancestors_are_refused() {
    use std::os::unix::fs::symlink;
    let (root, p) = fixture();
    let alias = root.path().join("alias.zip");
    symlink(&p.source, &alias).unwrap();
    assert!(preview_extract(&alias, &p.destination).is_err());
    let parent = root.path().join("link");
    symlink(root.path(), &parent).unwrap();
    assert!(preview_extract(&p.source, &parent.join("out")).is_err());
    let s = root.path().join("links.zip");
    let mut w = zip::ZipWriter::new(fs::File::create(&s).unwrap());
    w.add_symlink("link", "outside", zip::write::SimpleFileOptions::default())
        .unwrap();
    w.finish().unwrap();
    assert!(preview_extract(&s, &p.destination).is_err());
}
#[test]
fn cancellation_and_injected_io_failure_before_commit_leave_no_output() {
    for cancel_run in [true, false] {
        let (root, p) = fixture();
        let cancel = no();
        let r = apply_with_observer(&p, &cancel, |phase| {
            if phase == StagePhase::BeforePublication {
                if cancel_run {
                    cancel.store(true, Ordering::Release);
                } else {
                    return Err(ZipError::Io("injected ENOSPC".into()));
                }
            }
            Ok(())
        });
        assert!(r.is_err());
        assert!(!p.destination.exists());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}
#[test]
fn after_commit_failure_is_uncertain_and_never_removes_output() {
    let (_root, p) = fixture();
    let r = apply_with_observer(&p, &no(), |phase| {
        if phase == StagePhase::AfterPublication {
            return Err(ZipError::Io("injected directory sync failure".into()));
        }
        Ok(())
    });
    assert!(matches!(r, Err(ZipError::PublicationUncertain { .. })));
    assert_eq!(fs::read(&p.destination).unwrap(), b"approved A");
    assert_eq!(
        preview_extract(&p.source, &p.destination).unwrap_err(),
        ZipError::DestinationAlreadyExists
    );
}
#[test]
fn post_commit_foreign_replacement_is_never_deleted_or_reported_success() {
    let (root, p) = fixture();
    let saved = root.path().join("published-A");
    let r = apply_with_observer(&p, &no(), |phase| {
        if phase == StagePhase::AfterPublication {
            fs::rename(&p.destination, &saved).unwrap();
            fs::write(&p.destination, b"B").unwrap();
        }
        Ok(())
    });
    assert!(matches!(r, Err(ZipError::PublicationUncertain { .. })));
    assert_eq!(fs::read(saved).unwrap(), b"approved A");
    assert_eq!(fs::read(&p.destination).unwrap(), b"B");
}
#[test]
fn crc_corruption_and_excessive_declarations_refuse_with_tiny_fixtures() {
    let (root, p) = fixture();
    let mut b = fs::read(&p.source).unwrap();
    let payload = 30
        + u16::from_le_bytes(b[26..28].try_into().unwrap()) as usize
        + u16::from_le_bytes(b[28..30].try_into().unwrap()) as usize;
    b[payload] ^= 1;
    fs::write(&p.source, b).unwrap();
    assert!(preview_extract(&p.source, &p.destination).is_err());
    zip(&p.source, &[("game.bin", b"A")]);
    let mut b = fs::read(&p.source).unwrap();
    let cd = b.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
    b[22..26].copy_from_slice(&u32::MAX.to_le_bytes());
    b[cd + 24..cd + 28].copy_from_slice(&u32::MAX.to_le_bytes());
    fs::write(&p.source, b).unwrap();
    assert!(preview_extract(&p.source, &root.path().join("out")).is_err());
}
#[test]
fn two_reviewed_previews_cannot_clobber_one_destination() {
    let (_root, p) = fixture();
    let q = p.clone();
    let first = std::thread::spawn(move || apply_reviewed(&p, &no()));
    let second = std::thread::spawn(move || apply_reviewed(&q, &no()));
    let a = first.join().unwrap();
    let b = second.join().unwrap();
    assert!(a.is_ok() ^ b.is_ok());
}
#[test]
#[ignore = "fresh-process crash helper invoked only by parent with disposable fixture env"]
fn crash_worker() {
    let root = PathBuf::from(std::env::var_os("EMUWIZ_ZIP_CRASH_ROOT").unwrap());
    let point = std::env::var("EMUWIZ_ZIP_CRASH_POINT").unwrap();
    let p = preview_extract(&root.join("source.zip"), &root.join("output.bin")).unwrap();
    apply_with_observer(&p, &no(), |phase| {
        if format!("{phase:?}") == point {
            fs::write(root.join("paused"), b"ready").unwrap();
            loop {
                std::thread::park();
            }
        }
        Ok(())
    })
    .unwrap();
}
#[test]
fn fresh_process_sigkill_before_and_after_commit_preserves_truthful_recovery() {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    for point in [
        "EntryStaged",
        "BeforePublication",
        "PublishReady",
        "BeforeLink",
        "AfterPublication",
    ] {
        let (root, p) = fixture();
        let original = fs::read(&p.source).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash_worker", "--ignored", "--nocapture"])
            .env("EMUWIZ_ZIP_CRASH_ROOT", root.path())
            .env("EMUWIZ_ZIP_CRASH_POINT", point)
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let start = Instant::now();
        while !root.path().join("paused").exists() {
            assert!(start.elapsed() < Duration::from_secs(15));
            assert!(child.try_wait().unwrap().is_none());
            std::thread::sleep(Duration::from_millis(5));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(fs::read(&p.source).unwrap(), original);
        if point == "AfterPublication" {
            assert_eq!(fs::read(&p.destination).unwrap(), b"approved A");
            assert_eq!(
                preview_extract(&p.source, &p.destination).unwrap_err(),
                ZipError::DestinationAlreadyExists
            );
        } else {
            assert!(!p.destination.exists());
            assert!(preview_extract(&p.source, &p.destination).is_ok());
        }
        assert!(fs::read_dir(root.path()).unwrap().all(|e| {
            !e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".emuwiz-zip")
        }));
    }
}

#[test]
fn collision_after_final_validation_is_refused_by_the_actual_commit() {
    use std::sync::{Arc, Barrier};
    let (_root, p) = fixture();
    let q = p.clone();
    let barrier = Arc::new(Barrier::new(2));
    let other = barrier.clone();
    let a = std::thread::spawn(move || {
        apply_with_observer(&p, &no(), |phase| {
            if phase == StagePhase::BeforeLink {
                barrier.wait();
            }
            Ok(())
        })
    });
    let b = std::thread::spawn(move || {
        apply_with_observer(&q, &no(), |phase| {
            if phase == StagePhase::BeforeLink {
                other.wait();
            }
            Ok(())
        })
    });
    let a = a.join().unwrap();
    let b = b.join().unwrap();
    assert!(a.is_ok() ^ b.is_ok());
    assert!(matches!(
        a.err().or(b.err()),
        Some(ZipError::StalePreview(_))
    ));
}
#[test]
fn changing_source_after_final_validation_never_publishes_foreign_bytes() {
    let (_root, p) = fixture();
    let result = apply_with_observer(&p, &no(), |phase| {
        if phase == StagePhase::BeforeLink {
            zip(&p.source, &[("game.bin", b"foreign B!")]);
        }
        Ok(())
    });
    assert!(matches!(result, Err(ZipError::PublicationUncertain { .. })));
    assert_eq!(fs::read(&p.destination).unwrap(), b"approved A");
}
#[test]
fn replaced_destination_parent_refuses_or_reports_uncertainty_without_touching_foreign_tree() {
    for boundary in [StagePhase::BeforePublication, StagePhase::AfterPublication] {
        let (root, initial) = fixture();
        let parent = root.path().join("destination");
        let moved = root.path().join("relocated");
        fs::create_dir(&parent).unwrap();
        let p = preview_extract(&initial.source, &parent.join("output")).unwrap();
        let result = apply_with_observer(&p, &no(), |phase| {
            if phase == boundary {
                fs::rename(&parent, &moved).unwrap();
                fs::create_dir(&parent).unwrap();
                fs::write(parent.join("foreign"), b"B").unwrap();
            }
            Ok(())
        });
        assert_eq!(fs::read(parent.join("foreign")).unwrap(), b"B");
        assert!(!parent.join("output").exists());
        if boundary == StagePhase::AfterPublication {
            assert!(matches!(result, Err(ZipError::PublicationUncertain { .. })));
            assert_eq!(fs::read(moved.join("output")).unwrap(), b"approved A");
        } else {
            assert!(matches!(result, Err(ZipError::StalePreview(_))));
            assert!(!moved.join("output").exists());
        }
    }
}

#[test]
fn unsupported_filesystem_refuses_without_a_named_probe_or_output() {
    let (root, p) = fixture();
    let output = PathBuf::from(format!(
        "/proc/emuwiz-zip-never-created-{}",
        std::process::id()
    ));
    let reviewed = preview_extract(&p.source, &output).unwrap();
    assert!(!reviewed.can_apply());
    assert!(matches!(
        apply_reviewed(&reviewed, &no()),
        Err(ZipError::UnsupportedPublication { .. })
    ));
    assert!(!output.exists());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}
