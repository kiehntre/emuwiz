use super::*;
use crate::dat::rename_apply::writer_gate::tests::{add_process, fake_table};
use std::os::unix::fs::symlink;

struct Fixture {
    table: tempfile::TempDir,
    ours: tempfile::TempDir,
    peer: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let f = Self {
            table: fake_table("emuwiz-gui", 4_800_002, "82"),
            ours: tempfile::tempdir().unwrap(),
            peer: tempfile::tempdir().unwrap(),
        };
        for home in [f.ours.path(), f.peer.path()] {
            std::fs::create_dir_all(home.join(".config/emuwiz")).unwrap();
            std::fs::create_dir_all(home.join(".local/share/emuwiz/rename-transactions")).unwrap();
        }
        configure_process(f.table.path(), 4_800_002, f.peer.path());
        let own = std::process::id();
        add_process(f.table.path(), "emuwiz-gui", own, "83");
        std::fs::write(
            f.table.path().join(own.to_string()).join("cmdline"),
            b"emuwiz-gui\0",
        )
        .unwrap();
        f
    }

    fn process(&self) -> PathBuf {
        self.table.path().join("4800002")
    }

    fn env(&self) -> Environment {
        vec![("HOME".into(), self.ours.path().into())]
    }

    fn isolated(&self) -> bool {
        isolated(self.table.path(), 4_800_002, &self.env(), || {}).is_some()
    }

    fn peer_env(&self, extra: &str) {
        std::fs::write(
            self.process().join("environ"),
            format!("HOME={}\0{extra}", self.peer.path().display()),
        )
        .unwrap();
    }
}

pub(crate) fn configure_process(root: &Path, pid: u32, home: &Path) {
    let process = root.join(pid.to_string());
    std::fs::write(
        process.join("environ"),
        format!("HOME={}\0", home.display()),
    )
    .unwrap();
    std::fs::write(process.join("cmdline"), b"emuwiz-gui\0").unwrap();
    std::fs::create_dir(process.join("fd")).unwrap();
    symlink(home, process.join("cwd")).unwrap();
    std::fs::create_dir(process.join("ns")).unwrap();
    symlink(
        std::fs::read_link("/proc/self/ns/mnt").unwrap(),
        process.join("ns/mnt"),
    )
    .unwrap();
    symlink("/", process.join("root")).unwrap();
}

#[test]
fn separate_profiles_have_verified_disjoint_state() {
    let f = Fixture::new();
    assert!(f.isolated());
    // Explicit overrides alone cannot hide the historical HOME profile.
    let mut ours = f.env();
    ours.push((
        "EMUWIZ_DATA_HOME".into(),
        f.peer.path().join(".local/share/emuwiz").into(),
    ));
    assert!(isolated(f.table.path(), 4_800_002, &ours, || {}).is_none());
}

#[test]
fn different_home_with_shared_data_or_config_is_not_isolated() {
    let f = Fixture::new();
    for (key, tail) in [
        ("EMUWIZ_DATA_HOME", ".local/share/emuwiz"),
        ("EMUWIZ_CONFIG_HOME", ".config/emuwiz"),
    ] {
        f.peer_env(&format!("{key}={}\0", f.ours.path().join(tail).display()));
        assert!(!f.isolated(), "{key}");
    }
    f.peer_env(&format!(
        "XDG_DATA_HOME={}\0",
        f.ours.path().join(".local/share").display()
    ));
    assert!(!f.isolated());
}

#[test]
fn unreadable_incomplete_and_ambiguous_environment_fails_closed() {
    let f = Fixture::new();
    let path = f.process().join("environ");
    std::fs::remove_file(&path).unwrap();
    // EISDIR is deterministic even when tests run as root.
    std::fs::create_dir(&path).unwrap();
    assert!(!f.isolated());
    std::fs::remove_dir(&path).unwrap();
    for bytes in [
        b"HOME=/elsewhere".as_slice(),
        b"HOME=/a\0HOME=/b\0",
        b"HOME=relative\0",
        b"XDG_DATA_HOME=/a\0",
    ] {
        std::fs::write(&path, bytes).unwrap();
        assert!(!f.isolated());
    }
}

#[test]
fn aliases_overlaps_and_shared_database_inodes_fail_closed() {
    let f = Fixture::new();
    let ours = f.ours.path().join(".local/share/emuwiz");
    let alias = f.peer.path().join("alias");
    symlink(&ours, &alias).unwrap();
    f.peer_env(&format!("EMUWIZ_DATA_HOME={}\0", alias.display()));
    assert!(!f.isolated());
    for path in [
        ours.clone(),
        ours.join("nested"),
        f.ours.path().to_path_buf(),
    ] {
        f.peer_env(&format!("EMUWIZ_DATA_HOME={}\0", path.display()));
        assert!(!f.isolated());
    }
    f.peer_env("");
    let peer_data = f.peer.path().join(".local/share/emuwiz");
    symlink(&ours, peer_data.join("shared-journals")).unwrap();
    assert!(!f.isolated());
    std::fs::remove_file(peer_data.join("shared-journals")).unwrap();
    std::fs::write(ours.join("library.sqlite3"), b"synthetic database").unwrap();
    std::fs::hard_link(
        ours.join("library.sqlite3"),
        peer_data.join("library.sqlite3"),
    )
    .unwrap();
    assert!(!f.isolated());
}

#[test]
fn shared_open_journal_and_unknown_command_line_fail_closed() {
    let f = Fixture::new();
    let journal = f.ours.path().join(".local/share/emuwiz/journal");
    std::fs::write(&journal, b"synthetic journal").unwrap();
    symlink(&journal, f.process().join("fd/3")).unwrap();
    assert!(!f.isolated());
    std::fs::remove_file(f.process().join("fd/3")).unwrap();
    assert!(f.isolated());
    for bytes in [
        b"emuwiz-gui\0--data-home=/shared\0".as_slice(),
        b"emuwiz-cli\0undo\0/shared/journal\0",
    ] {
        std::fs::write(f.process().join("cmdline"), bytes).unwrap();
        assert!(!f.isolated());
    }
    std::fs::remove_file(f.process().join("cmdline")).unwrap();
    assert!(!f.isolated());
}

#[test]
fn exit_pid_reuse_and_exec_during_inspection_invalidate_isolation() {
    for condition in ["exit", "reuse", "exec"] {
        let f = Fixture::new();
        assert!(f.isolated());
        let result = isolated(f.table.path(), 4_800_002, &f.env(), || match condition {
            "exit" => std::fs::remove_dir_all(f.process()).unwrap(),
            "reuse" => std::fs::write(
                f.process().join("stat"),
                "4800002 (emuwiz-gui) S 1 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 999 0 0",
            )
            .unwrap(),
            "exec" => {
                let exe = f.table.path().join("replacement");
                std::fs::write(&exe, b"replacement").unwrap();
                std::fs::remove_file(f.process().join("exe")).unwrap();
                symlink(exe, f.process().join("exe")).unwrap();
            }
            _ => unreachable!(),
        });
        assert!(result.is_none(), "{condition}");
    }
}

#[test]
fn changed_tree_and_unreadable_fd_table_invalidate_isolation() {
    let f = Fixture::new();
    assert!(
        isolated(f.table.path(), 4_800_002, &f.env(), || {
            symlink(
                f.ours.path(),
                f.peer.path().join(".local/share/emuwiz/shared"),
            )
            .unwrap();
        })
        .is_none()
    );
    std::fs::remove_file(f.peer.path().join(".local/share/emuwiz/shared")).unwrap();
    std::fs::remove_dir(f.process().join("fd")).unwrap();
    std::fs::write(f.process().join("fd"), b"unreadable as a directory").unwrap();
    assert!(!f.isolated());
}

#[test]
fn replacement_process_with_same_pid_start_tick_and_executable_is_not_isolated() {
    let f = Fixture::new();
    assert!(
        isolated(f.table.path(), 4_800_002, &f.env(), || {
            let old = f.table.path().join("exited-process");
            std::fs::rename(f.process(), &old).unwrap();
            std::fs::create_dir(f.process()).unwrap();
            std::fs::copy(old.join("stat"), f.process().join("stat")).unwrap();
            symlink(
                std::fs::read_link(old.join("exe")).unwrap(),
                f.process().join("exe"),
            )
            .unwrap();
            configure_process(f.table.path(), 4_800_002, f.peer.path());
        })
        .is_none()
    );
}

#[test]
fn namespace_chroot_and_command_changes_invalidate_evidence() {
    for condition in ["namespace", "chroot", "command"] {
        let f = Fixture::new();
        assert!(
            isolated(f.table.path(), 4_800_002, &f.env(), || {
                match condition {
                    "namespace" => {
                        std::fs::remove_file(f.process().join("ns/mnt")).unwrap();
                        symlink("mnt:[different]", f.process().join("ns/mnt")).unwrap();
                    }
                    "chroot" => {
                        std::fs::remove_file(f.process().join("root")).unwrap();
                        symlink(f.peer.path(), f.process().join("root")).unwrap();
                    }
                    "command" => {
                        std::fs::write(f.process().join("cmdline"), b"emuwiz-gui\0/shared\0")
                            .unwrap()
                    }
                    _ => unreachable!(),
                }
            })
            .is_none(),
            "{condition}"
        );
    }
}

#[test]
fn home_alone_without_verified_profile_directories_is_insufficient() {
    let f = Fixture::new();
    std::fs::remove_dir_all(f.peer.path().join(".config")).unwrap();
    assert!(!f.isolated());
    std::fs::create_dir_all(f.peer.path().join(".config/emuwiz")).unwrap();
    std::fs::remove_dir_all(f.peer.path().join(".local/share")).unwrap();
    assert!(!f.isolated());
}
