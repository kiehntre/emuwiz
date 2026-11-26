//! Integration test binaries are isolated too: they link the library without
//! `cfg(test)`, so they rely on the `test-support` dev-feature and the
//! per-binary constructor from `install_test_environment!`.
archivefs_core::install_test_environment!();

use std::path::PathBuf;

#[test]
fn an_integration_test_binary_runs_in_a_private_home() {
    let Some(root) = archivefs_core::test_environment::isolated_root() else {
        assert_eq!(
            std::env::var("EMUWIZ_TEST_REAL_ENVIRONMENT").as_deref(),
            Ok("1"),
            "isolation was not installed in this integration test binary"
        );
        return;
    };
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    assert!(home.starts_with(root), "{home:?} escapes {root:?}");
    assert!(
        archivefs_core::app_dirs::data_dir()
            .unwrap()
            .starts_with(root)
    );
    for name in archivefs_core::test_environment::cleared_variables() {
        assert!(std::env::var_os(name).is_none(), "{name} must be cleared");
    }
}

#[test]
fn separate_test_processes_get_separate_trees() {
    let Some(root) = archivefs_core::test_environment::isolated_root() else {
        return;
    };
    // The tree is keyed by executable, pid, start time and an exclusive
    // counter, and owned through a lock this process holds for its lifetime,
    // so two concurrently running test binaries (or two runs, even with the
    // same pid in different PID namespaces) can never share mutable state.
    let name = root.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.contains(&format!("-{}-", std::process::id())),
        "{name}"
    );
    let mut lock_name = root.file_name().unwrap().to_os_string();
    lock_name.push(".lock");
    let lock = std::fs::File::open(root.with_file_name(lock_name)).unwrap();
    use std::os::fd::AsRawFd;
    // SAFETY: flock on a descriptor owned by this function.
    let taken = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    assert!(
        !taken,
        "the owner lock on {name} must be held by this process"
    );
}
