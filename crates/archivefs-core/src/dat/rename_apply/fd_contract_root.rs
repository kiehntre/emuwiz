//! Local root capability measurements. No filesystem-name-based eligibility.
use super::*;
use std::os::fd::AsRawFd;

#[test]
fn local_root_capabilities_and_bounded_copy_cost() {
    let f = fixture("root-probe-");
    let source = File::open(f.root.join("old.bin")).unwrap();
    let copy_path = f.root.join("reflink-probe");
    let copy = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&copy_path)
        .unwrap();
    let before_blocks = source.metadata().unwrap().blocks();
    let started = std::time::Instant::now();
    let rc = unsafe { libc::ioctl(copy.as_raw_fd(), libc::FICLONE, source.as_raw_fd()) };
    let clone_errno = if rc == 0 {
        None
    } else {
        Some(io::Error::last_os_error().raw_os_error())
    };
    if rc == 0 {
        copy.sync_all().unwrap();
        File::open(&f.root).unwrap().sync_all().unwrap();
        assert_eq!(fs::read(&copy_path).unwrap(), b"AAAA");
        assert_ne!(
            copy.metadata().unwrap().ino(),
            source.metadata().unwrap().ino()
        );
    }
    let mut generation: libc::c_long = 0;
    let generation_rc =
        unsafe { libc::ioctl(source.as_raw_fd(), libc::FS_IOC_GETVERSION, &mut generation) };
    let generation_error = if generation_rc == 0 {
        None
    } else {
        io::Error::last_os_error().raw_os_error()
    };
    let report = json!({"clone_rc":rc,"clone_errno":clone_errno,"clone_elapsed_ns":started.elapsed().as_nanos(),
        "source_allocated_bytes":before_blocks*512,"copy_allocated_bytes":copy.metadata().unwrap().blocks()*512,
        "generation_rc":generation_rc,"generation_errno":generation_error,"generation":generation,
        "metadata_allocation":"not separately measurable by stat; shared extents not inferred",
        "cow_worst_case":"up to logical file size per independently modified clone; not zero-cost",
        "root_eligible":false,"reason":"capabilities alone do not approve Contract 1-I"});
    fs::write(
        f.root.join("capabilities.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    println!("ROOT CAPABILITIES {report}");
    // Bounded real non-reflink copy: 8 MiB, no sparse-size masquerade.
    let large = f.root.join("large");
    fs::write(&large, vec![0x5a; 8 * 1024 * 1024]).unwrap();
    let started = std::time::Instant::now();
    let destination = f.root.join("large-copy");
    fs::copy(&large, &destination).unwrap();
    File::open(&destination).unwrap().sync_all().unwrap();
    assert!(
        crate::dat::rename_apply::identity::content_identity_matches(
            &capture_identity(&large).unwrap(),
            &capture_identity(&destination).unwrap()
        )
    );
    println!(
        "BOUNDED COPY logical={} allocated={} elapsed_ms={}",
        fs::metadata(&destination).unwrap().len(),
        fs::metadata(&destination).unwrap().blocks() * 512,
        started.elapsed().as_millis()
    );
}

#[test]
fn lease_worker() {
    let Some(path) = std::env::var_os("EMUWIZ_CONTRACT_LEASE") else {
        return;
    };
    // In a dedicated process only; never change the main test runner's signal handler.
    unsafe {
        libc::signal(libc::SIGIO, libc::SIG_IGN);
    }
    let f = File::open(&path).unwrap();
    let rc = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_SETLEASE, libc::F_RDLCK) };
    println!(
        "LEASE {} {:?}",
        rc,
        if rc == 0 {
            None
        } else {
            io::Error::last_os_error().raw_os_error()
        }
    );
    if rc == 0 {
        assert_eq!(
            unsafe { libc::fcntl(f.as_raw_fd(), libc::F_SETLEASE, libc::F_UNLCK) },
            0
        );
    }
}

#[test]
fn leases_characterize_external_writer_and_shared_mapping() {
    let f = fixture("leases-");
    let path = f.root.join("old.bin");
    let run = || {
        let out = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("{TEST_PREFIX}cleanup::contract_tests::root_probes::lease_worker"),
                "--nocapture",
            ])
            .env("EMUWIZ_CONTRACT_LEASE", &path)
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    };
    let alone = run();
    let writer = OpenOptions::new().write(true).open(&path).unwrap();
    let writable_fd = run();
    assert!(writable_fd.contains("LEASE -1 Some(11)"), "{writable_fd}");
    drop(writer);
    let mapping_file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            4,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            mapping_file.as_raw_fd(),
            0,
        )
    };
    assert_ne!(ptr, libc::MAP_FAILED);
    drop(mapping_file);
    let mapped = run();
    assert!(mapped.contains("LEASE -1 Some(11)"), "{mapped}");
    assert_eq!(unsafe { libc::munmap(ptr, 4) }, 0);
    fs::write(
        f.root.join("lease-results.txt"),
        format!("ALONE\n{alone}\nO_WRONLY\n{writable_fd}\nMAP_SHARED\n{mapped}"),
    )
    .unwrap();
    // This measures current conflicts only. It is not permanent writer exclusion.
}

#[test]
fn actual_root_eligibility_and_holding_inventory() {
    let f = fixture("eligibility-");
    let report = contract::capability::probe(&f.root).unwrap();
    assert_eq!(report.root_identity.0, fs::metadata(&f.root).unwrap().dev());
    assert_eq!(report.filesystem_type, "ext4");
    let alternate = tempfile::Builder::new()
        .prefix("emuwiz-contract-ineligible-")
        .tempdir_in("/dev/shm")
        .unwrap();
    assert!(contract::capability::probe(alternate.path()).is_err());
    let mut tx = enrolled_transaction(&f);
    assert!(
        contract::undo_entry(
            &f.root,
            &mut tx,
            0,
            &mut |tx| save(&f, tx),
            &mut |edge| {
                if edge == Edge::CleanupRecorded {
                    return Err(io::Error::other("pause"));
                }
                Ok(())
            },
            Fault::None
        )
        .is_err()
    );
    let inventory = contract::capability::inventory(&f.root, &tx).unwrap();
    assert_eq!(inventory["retained_entries"], 1);
    assert_eq!(inventory["unique_inodes"], 1);
    assert_eq!(inventory["entries"][0]["shares_known_library_inode"], true);
    assert_eq!(inventory["incremental_data_allocation_for_hardlink"], 0);
    assert!(inventory["reclaimable_bytes"].is_null());
    fs::write(
        f.root.join("holding-inventory.json"),
        serde_json::to_vec_pretty(&inventory).unwrap(),
    )
    .unwrap();
}

#[test]
fn bounded_inode_reuse_observations_do_not_authorize_lost_anchor_recovery() {
    let f = fixture("inode-reuse-");
    let path = f.root.join("reuse");
    fs::write(&path, b"same").unwrap();
    let original = fs::metadata(&path).unwrap().ino();
    fs::remove_file(&path).unwrap();
    let mut observed = None;
    for index in 0..2000 {
        fs::write(&path, b"same").unwrap();
        if fs::metadata(&path).unwrap().ino() == original {
            observed = Some(index);
            break;
        }
        fs::remove_file(&path).unwrap();
    }
    println!(
        "BOUNDED INODE REUSE first_reuse={observed:?}; missing trusted anchors always refuse automatic recovery"
    );
    fs::write(f.root.join("inode-reuse.json"),serde_json::to_vec(&json!({"iterations_bound":2000,"first_reuse":observed,"absence_is_not_proof_of_no_reuse":true})).unwrap()).unwrap();
}
