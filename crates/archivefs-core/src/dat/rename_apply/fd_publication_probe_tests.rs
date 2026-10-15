use super::*;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::process::Command;

const TEST_PREFIX: &str = "dat::rename_apply::identity::fd_publication_probe::tests::";

struct Fixture {
    root: PathBuf,
    a: ObjectIdentity,
    b: ObjectIdentity,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let root = tempfile::Builder::new()
            .prefix(label)
            .tempdir_in(
                std::env::var_os("EMUWIZ_FDP_FIXTURES")
                    .expect("explicit disposable fixture root required"),
            )
            .unwrap()
            .keep();
        fs::create_dir(root.join("holding")).unwrap();
        fs::create_dir(root.join("destination")).unwrap();
        fs::write(root.join("holding/object"), b"AAAA").unwrap();
        fs::write(root.join("e.bin"), b"BBBB").unwrap();
        let a = capture_identity(&root.join("holding/object")).unwrap();
        let b = capture_identity(&root.join("e.bin")).unwrap();
        assert_ne!(a.ino, b.ino);
        println!(
            "FIXTURE {} A=({}, {}) B=({}, {})",
            root.display(),
            a.dev,
            a.ino,
            b.dev,
            b.ino
        );
        Self { root, a, b }
    }
    fn source(&self) -> PathBuf {
        self.root.join("holding/object")
    }
    fn target(&self) -> PathBuf {
        self.root.join("destination/s.bin")
    }
    fn publisher(&self) -> Publisher {
        Publisher::open(&self.root, &self.source(), &self.target(), &self.a).unwrap()
    }
    fn assert_a(&self, path: &Path) {
        let actual = capture_identity(path).unwrap();
        assert!(identity_matches(&self.a, &actual));
        // Essential assertion retained from the failed pathname experiment.
        assert!(
            !identity_matches(&self.b, &actual),
            "SAFETY VIOLATION: EmuWiz published foreign B as A; a later error is not prevention"
        );
        assert_eq!(fs::read(path).unwrap(), b"AAAA");
    }
}

fn worker(root: &Path) {
    assert!(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("{TEST_PREFIX}competitor"),
                "--nocapture"
            ])
            .env("EMUWIZ_FDP_COMPETITOR", root)
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn competitor() {
    let Some(root) = std::env::var_os("EMUWIZ_FDP_COMPETITOR") else {
        return;
    };
    let root = PathBuf::from(root);
    fs::rename(root.join("holding/object"), root.join("t.bin")).unwrap();
    fs::rename(root.join("e.bin"), root.join("holding/object")).unwrap();
    println!(
        "COMPETITOR pid={} replaced holding entry",
        std::process::id()
    );
}

#[test]
fn t0_ordinary_user_fd_link_is_exact_and_adds_an_alias() {
    let f = Fixture::new("t0-");
    let status = fs::read_to_string("/proc/self/status").unwrap();
    let uid = status.lines().find(|s| s.starts_with("Uid:")).unwrap();
    let cap = status.lines().find(|s| s.starts_with("CapEff:")).unwrap();
    assert_ne!(uid.split_whitespace().nth(1).unwrap(), "0");
    assert_eq!(cap.split_whitespace().nth(1).unwrap(), "0000000000000000");
    println!(
        "{uid}; {cap}; protected_hardlinks={}",
        fs::read_to_string("/proc/sys/fs/protected_hardlinks")
            .unwrap()
            .trim()
    );
    let p = f.publisher();
    // SAFETY: read-only descriptor flag query on a live File.
    assert_ne!(
        unsafe { libc::fcntl(p.file.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
        0
    );
    assert!(matches!(
        p.publish(|| {}).unwrap(),
        Publication::LinkedVerified
    ));
    f.assert_a(&f.target());
    f.assert_a(&f.source());
    assert_eq!(p.file.metadata().unwrap().nlink(), 2);
}

#[test]
fn t0_eexist_never_overwrites_destination() {
    let f = Fixture::new("occupied-");
    let p = f.publisher();
    let result = p
        .publish(|| fs::write(f.target(), b"CCCC").unwrap())
        .unwrap();
    assert!(
        matches!(result, Publication::LinkError(ref e) if e.raw_os_error()==Some(libc::EEXIST))
    );
    assert_eq!(fs::read(f.target()).unwrap(), b"CCCC");
    f.assert_a(&f.source());
}

#[test]
fn required_invariant_same_uid_must_not_publish_b_after_held_name_swap() {
    let f = Fixture::new("decisive-");
    let p = f.publisher();
    assert!(matches!(
        p.publish(|| worker(&f.root)).unwrap(),
        Publication::LinkedVerified
    ));
    f.assert_a(&f.target());
    f.assert_a(&f.root.join("t.bin"));
    assert!(identity_matches(
        &f.b,
        &capture_identity(&f.source()).unwrap()
    ));
    println!(
        "FD PUBLICATION A_inode={} published_inode={} B_inode={} bytes={:?}",
        f.a.ino,
        fs::metadata(f.target()).unwrap().ino(),
        f.b.ino,
        fs::read(f.target()).unwrap()
    );
}

#[test]
fn removed_before_open_refuses() {
    let f = Fixture::new("before-open-");
    fs::remove_file(f.source()).unwrap(); // deliberate competitor fixture action
    assert!(Publisher::open(&f.root, &f.source(), &f.target(), &f.a).is_err());
    assert!(!f.target().exists());
}

#[test]
fn t0_unlinked_last_name_refuses_without_resurrecting_or_publishing_b() {
    let f = Fixture::new("last-link-");
    let p = f.publisher();
    fs::remove_file(f.source()).unwrap();
    fs::rename(f.root.join("e.bin"), f.source()).unwrap();
    assert_eq!(p.file.metadata().unwrap().nlink(), 0);
    let result = p.publish(|| {}).unwrap();
    assert!(
        matches!(result, Publication::LinkError(ref e) if e.raw_os_error()==Some(libc::ENOENT))
    );
    assert!(!f.target().exists());
    assert_eq!(fs::read(f.source()).unwrap(), b"BBBB");
}

#[test]
fn removed_source_with_retained_hardlink_anchor_can_publish_a() {
    let f = Fixture::new("anchor-");
    let p = f.publisher();
    fs::hard_link(f.source(), f.root.join("anchor")).unwrap();
    fs::remove_file(f.source()).unwrap();
    assert!(matches!(
        p.publish(|| {}).unwrap(),
        Publication::LinkedVerified
    ));
    f.assert_a(&f.target());
    f.assert_a(&f.root.join("anchor"));
    assert_eq!(p.file.metadata().unwrap().nlink(), 2);
}

#[test]
fn source_leaf_symlink_is_not_followed() {
    let f = Fixture::new("symlink-");
    fs::rename(f.source(), f.root.join("anchor")).unwrap();
    std::os::unix::fs::symlink(f.root.join("anchor"), f.source()).unwrap();
    assert!(Publisher::open(&f.root, &f.source(), &f.target(), &f.a).is_err());
    assert!(!f.target().exists());
}

#[test]
fn reviewed_hardlink_alias_opens_the_same_a_inode() {
    let f = Fixture::new("alias-");
    fs::hard_link(f.source(), f.root.join("alias")).unwrap();
    let p = Publisher::open(&f.root, &f.root.join("alias"), &f.target(), &f.a).unwrap();
    assert!(matches!(
        p.publish(|| {}).unwrap(),
        Publication::LinkedVerified
    ));
    f.assert_a(&f.target());
    assert_eq!(p.file.metadata().unwrap().nlink(), 3);
}

#[test]
fn substitution_before_verification_is_rejected_even_for_same_bytes() {
    let f = Fixture::new("preverify-");
    fs::rename(f.source(), f.root.join("anchor")).unwrap();
    fs::write(f.source(), b"AAAA").unwrap();
    assert!(Publisher::open(&f.root, &f.source(), &f.target(), &f.a).is_err());
    assert!(!f.target().exists());
}

#[test]
fn existing_writable_descriptor_changes_content_not_publication_inode() {
    let f = Fixture::new("writable-");
    let writable = OpenOptions::new().write(true).open(f.source()).unwrap();
    let p = f.publisher();
    let result = p
        .publish(|| writable.write_all_at(b"EDIT", 0).unwrap())
        .unwrap();
    assert!(matches!(result, Publication::LinkedNeedsReview(ref e) if e.contains("content")));
    assert_eq!(fs::metadata(f.target()).unwrap().ino(), f.a.ino);
    assert_ne!(fs::metadata(f.target()).unwrap().ino(), f.b.ino);
    assert_eq!(fs::read(f.target()).unwrap(), b"EDIT");
}

#[test]
fn destination_directory_replaced_before_check_refuses() {
    let f = Fixture::new("directory-before-");
    let p = f.publisher();
    fs::rename(f.root.join("destination"), f.root.join("saved-directory")).unwrap();
    fs::create_dir(f.root.join("destination")).unwrap();
    assert!(p.publish(|| {}).is_err());
    assert!(!f.target().exists());
    assert!(!f.root.join("saved-directory/s.bin").exists());
}

#[test]
fn destination_directory_replaced_after_check_is_bound_but_unresolved() {
    let f = Fixture::new("directory-during-");
    let p = f.publisher();
    let result = p
        .publish(|| {
            fs::rename(f.root.join("destination"), f.root.join("saved-directory")).unwrap();
            std::os::unix::fs::symlink(f.root.join("holding"), f.root.join("destination")).unwrap();
        })
        .unwrap();
    assert!(matches!(result, Publication::LinkedNeedsReview(_)));
    f.assert_a(&f.root.join("saved-directory/s.bin"));
    assert!(!f.root.join("holding/s.bin").exists());
}

#[test]
fn t0_cross_filesystem_link_returns_exdev() {
    let f = Fixture::new("cross-fs-");
    let p = f.publisher();
    let other = tempfile::Builder::new()
        .prefix("emuwiz-fdp-")
        .tempdir_in("/dev/shm")
        .unwrap();
    let dir = File::open(other.path()).unwrap();
    assert_ne!(
        dir.metadata().unwrap().dev(),
        p.file.metadata().unwrap().dev()
    );
    let e = link_descriptor(&p.file, &dir, c"target").unwrap_err();
    assert_eq!(e.raw_os_error(), Some(libc::EXDEV));
    assert!(!other.path().join("target").exists());
}

#[test]
fn invalid_destination_component_is_refused() {
    let f = Fixture::new("component-");
    let p = f.publisher();
    assert_eq!(
        link_descriptor(&p.file, &p.destination_dir, c"../escaped")
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EINVAL)
    );
}

#[test]
fn later_external_replacement_is_not_emuwiz_publication() {
    let f = Fixture::new("later-replacement-");
    let p = f.publisher();
    assert!(matches!(
        p.publish(|| {}).unwrap(),
        Publication::LinkedVerified
    ));
    f.assert_a(&f.target());
    fs::rename(f.target(), f.root.join("published-a")).unwrap();
    fs::rename(f.root.join("e.bin"), f.target()).unwrap();
    assert_eq!(
        fs::metadata(f.root.join("published-a")).unwrap().ino(),
        f.a.ino
    );
    assert_eq!(fs::metadata(f.target()).unwrap().ino(), f.b.ino);
}

#[test]
fn retirement_expected_name_retains_anchor_never_claims_complete_undo() {
    let f = Fixture::new("retire-a-");
    assert!(matches!(
        f.publisher().publish(|| {}).unwrap(),
        Publication::LinkedVerified
    ));
    let held = f.root.join("holding/retired");
    assert_eq!(
        retain_old_name(&f.root, &f.source(), &held, &f.a, || {}, || {}).unwrap(),
        Retirement::ExpectedAnchorRetained
    );
    f.assert_a(&held);
    f.assert_a(&f.target());
    assert!(!f.source().exists());
    assert_eq!(fs::metadata(&held).unwrap().nlink(), 2);
}

#[test]
fn retirement_foreign_name_is_linked_back_without_deletion() {
    let f = Fixture::new("retire-b-");
    assert!(matches!(
        f.publisher().publish(|| worker(&f.root)).unwrap(),
        Publication::LinkedVerified
    ));
    let held = f.root.join("holding/retired");
    assert_eq!(
        retain_old_name(&f.root, &f.source(), &held, &f.a, || {}, || {}).unwrap(),
        Retirement::ForeignLinkedBackNeedsReview
    );
    f.assert_a(&f.target());
    assert!(identity_matches(&f.b, &capture_identity(&held).unwrap()));
    assert!(identity_matches(
        &f.b,
        &capture_identity(&f.source()).unwrap()
    ));
}

#[test]
fn retirement_missing_old_name_preserves_published_a() {
    let f = Fixture::new("retire-missing-");
    assert!(matches!(
        f.publisher().publish(|| {}).unwrap(),
        Publication::LinkedVerified
    ));
    fs::rename(f.source(), f.root.join("saved-a")).unwrap();
    assert_eq!(
        retain_old_name(
            &f.root,
            &f.source(),
            &f.root.join("holding/retired"),
            &f.a,
            || {},
            || {}
        )
        .unwrap(),
        Retirement::SourceMissing
    );
    f.assert_a(&f.target());
}

#[test]
fn retirement_blocked_foreign_return_requires_manual_review() {
    let f = Fixture::new("retire-blocked-");
    assert!(matches!(
        f.publisher().publish(|| worker(&f.root)).unwrap(),
        Publication::LinkedVerified
    ));
    let held = f.root.join("holding/retired");
    assert_eq!(
        retain_old_name(
            &f.root,
            &f.source(),
            &held,
            &f.a,
            || {},
            || fs::write(f.source(), b"CCCC").unwrap()
        )
        .unwrap(),
        Retirement::ManualReview
    );
    f.assert_a(&f.target());
    assert!(identity_matches(&f.b, &capture_identity(&held).unwrap()));
    assert_eq!(fs::read(f.source()).unwrap(), b"CCCC");
}

#[test]
fn retirement_held_entry_swap_does_not_delete_or_return_the_substitute() {
    let f = Fixture::new("retire-held-swap-");
    assert!(matches!(
        f.publisher().publish(|| worker(&f.root)).unwrap(),
        Publication::LinkedVerified
    ));
    let held = f.root.join("holding/retired");
    assert_eq!(
        retain_old_name(
            &f.root,
            &f.source(),
            &held,
            &f.a,
            || {},
            || {
                fs::rename(&held, f.root.join("saved-b")).unwrap();
                fs::write(&held, b"CCCC").unwrap();
            }
        )
        .unwrap(),
        Retirement::ForeignLinkedBackNeedsReview
    );
    f.assert_a(&f.target());
    assert_eq!(fs::read(&held).unwrap(), b"CCCC");
    assert!(identity_matches(
        &f.b,
        &capture_identity(&f.source()).unwrap()
    ));
    assert!(identity_matches(
        &f.b,
        &capture_identity(&f.root.join("saved-b")).unwrap()
    ));
    // The return result does NOT certify that the holding slot still names B.
    // Every retirement outcome still requires review; nothing is unlinked.
}
