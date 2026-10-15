//! One explicitly invoked bounded run; no widening adversary catalogue.
use super::*;
use std::cell::RefCell;
use std::process::{Child, ChildStdin, ChildStdout};
use std::rc::Rc;

#[test]
fn external_loop() {
    if std::env::var_os("EMUWIZ_CONTRACT_EXTERNAL_LOOP").is_none() {
        return;
    }
    println!("EXT-READY");
    std::io::stdout().flush().unwrap();
    let mut pinned = std::collections::HashMap::<PathBuf, std::fs::File>::new();
    for line in std::io::stdin().lock().lines() {
        let v: serde_json::Value = serde_json::from_str(&line.unwrap()).unwrap();
        let root = Path::new(v["root"].as_str().unwrap());
        let action = v["action"].as_str().unwrap();
        let result = if action == "pin-old" {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(root.join("old.bin"))
                .unwrap();
            assert!(pinned.insert(root.to_path_buf(), file).is_none());
            Ok(())
        } else if action == "write-dest" {
            use std::os::unix::fs::FileExt;
            let file = pinned
                .remove(root)
                .expect("write adversary requires its pre-existing FD");
            file.write_all_at(b"XXXX", 0).and_then(|()| file.sync_all())
        } else {
            super::super::perform_external(root, action)
        };
        fs::write(
            root.join(format!("external-{action}.txt")),
            format!("{:?}", result.err().and_then(|e| e.raw_os_error())),
        )
        .unwrap();
        println!("EXT-DONE");
        std::io::stdout().flush().unwrap();
    }
}
struct External {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
}
impl External {
    fn new() -> Self {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("{TEST_PREFIX}cleanup::contract_tests::stress::external_loop"),
                "--nocapture",
            ])
            .env("EMUWIZ_CONTRACT_EXTERNAL_LOOP", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut value = Self {
            input: child.stdin.take(),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
        };
        value.until("EXT-READY");
        value
    }
    fn until(&mut self, target: &str) {
        loop {
            let mut line = String::new();
            assert!(self.output.read_line(&mut line).unwrap() > 0);
            if line.trim() == target {
                break;
            }
        }
    }
    fn act(&mut self, root: &Path, action: &str) {
        writeln!(
            self.input.as_mut().unwrap(),
            "{}",
            json!({"root":root,"action":action})
        )
        .unwrap();
        self.input.as_mut().unwrap().flush().unwrap();
        self.until("EXT-DONE");
    }
}
impl Drop for External {
    fn drop(&mut self) {
        drop(self.input.take());
        let _ = self.child.wait();
    }
}
fn quiet(label: &str) -> Fixture {
    let root = tempfile::Builder::new()
        .prefix(label)
        .tempdir_in(std::env::var_os("EMUWIZ_FDP_FIXTURES").unwrap())
        .unwrap()
        .keep();
    fs::create_dir(root.join("holding")).unwrap();
    fs::create_dir(root.join("destination")).unwrap();
    fs::write(root.join("old.bin"), b"AAAA").unwrap();
    fs::write(root.join("e.bin"), b"BBBB").unwrap();
    fs::write(root.join("victim.bin"), b"CCCC").unwrap();
    Fixture {
        a: capture_identity(&root.join("old.bin")).unwrap(),
        b: capture_identity(&root.join("e.bin")).unwrap(),
        root,
    }
}
fn objects(f: &Fixture, tx: Option<&RenameTransaction>) -> Vec<(u64, Vec<u8>, PathBuf)> {
    let mut paths = vec![
        f.root.join("old.bin"),
        f.target(),
        f.root.join("e.bin"),
        f.root.join("t.bin"),
        f.root.join("victim.bin"),
    ];
    if let Some(tx) = tx {
        let (h, s) = contract::paths(&f.root, tx, 0).unwrap();
        paths.extend([h, s, contract::anchor::path(&f.root, tx, 0).unwrap()]);
    }
    paths
        .into_iter()
        .filter_map(|p| {
            fs::symlink_metadata(&p)
                .ok()
                .filter(|m| m.is_file())
                .map(|m| (m.ino(), fs::read(&p).unwrap(), p))
        })
        .collect()
}

#[test]
#[ignore = "Explicit bounded 10,000-per-adversary Gate 1 run; executed separately with --ignored"]
fn bounded_10000_per_adversary() {
    let stop = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|scope| {
        for shard in 0..8 {
            let stop = &stop;
            scope.spawn(move || stress_shard(shard, stop));
        }
    });
}
fn stress_shard(shard: usize, stop: &std::sync::atomic::AtomicBool) {
    struct AbortOnFailure<'a>(&'a std::sync::atomic::AtomicBool);
    impl Drop for AbortOnFailure<'_> {
        fn drop(&mut self) {
            if std::thread::panicking() {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
    }
    let _abort = AbortOnFailure(stop);
    let worker = Rc::new(RefCell::new(External::new()));
    let started = std::time::Instant::now();
    for action in [
        "delete-dest",
        "replace-dest",
        "rename-dest",
        "write-dest",
        "overwrite-b",
        "substitute",
    ] {
        for iteration in (shard..10_000).step_by(8) {
            if stop.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            if (iteration - shard).is_multiple_of(1000) {
                let root = std::env::var_os("EMUWIZ_FDP_FIXTURES").unwrap();
                let name = CString::new(root.as_bytes()).unwrap();
                let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
                assert_eq!(
                    unsafe { libc::statvfs(name.as_ptr(), stats.as_mut_ptr()) },
                    0
                );
                let stats = unsafe { stats.assume_init() };
                assert!(
                    stats.f_bavail.saturating_mul(stats.f_frsize) >= 8 * 1024 * 1024 * 1024,
                    "8 GiB free-space guard"
                );
            }
            let baseline = quiet("stress-control-");
            let candidate = quiet("stress-r2-");
            let b_c = fs::metadata(baseline.root.join("victim.bin"))
                .unwrap()
                .ino();
            let r_c = fs::metadata(candidate.root.join("victim.bin"))
                .unwrap()
                .ino();
            if action == "write-dest" {
                worker.borrow_mut().act(&baseline.root, "pin-old");
                worker.borrow_mut().act(&candidate.root, "pin-old");
            }
            if action == "substitute" {
                let w = worker.clone();
                let root = baseline.root.clone();
                rollback::BEFORE_REVERSE_RENAME.with(|hook| {
                    *hook.borrow_mut() = Some(Box::new(move || {
                        w.borrow_mut().act(&root, "substitute");
                        rollback::AFTER_REVERSE_RENAME.with(|hook| {
                            *hook.borrow_mut() =
                                Some(Box::new(move || w.borrow_mut().act(&root, "overwrite-c")));
                        });
                    }))
                });
                assert_eq!(ordinary(&baseline).state, TransactionState::RollbackFailed);
                assert_eq!(
                    fs::read_to_string(baseline.root.join("external-overwrite-c.txt")).unwrap(),
                    format!("{:?}", Some(libc::ENOENT))
                );
                crate::dat::rename_apply::dirsync::set_test_hook(None);
            } else {
                assert_eq!(ordinary(&baseline).state, TransactionState::RolledBack);
                worker.borrow_mut().act(&baseline.root, action);
            }
            fs::create_dir(candidate.root.join("journals")).unwrap();
            let mut tx = enrolled_transaction(&candidate);
            tx.entries[0]
                .unknown
                .insert(contract::anchor::ENROLLED.into(), json!(true));
            // The fixture represents an already-applied transaction. Establish
            // the same trusted enrollment anchor before the adversary starts.
            let mut enrollment = tx.clone();
            let entry = &mut enrollment.entries[0];
            std::mem::swap(&mut entry.source_path, &mut entry.destination_path);
            contract::anchor::Setup::prepare(&candidate.root, &enrollment)
                .unwrap()
                .publish()
                .unwrap();
            save(&candidate, &mut tx).unwrap();
            contract::undo_entry(
                &candidate.root,
                &mut tx,
                0,
                &mut |tx| {
                    if action == "substitute"
                        && contract::evidence(tx, 0).unwrap().unwrap().phase == Phase::ForeignHeld
                    {
                        worker.borrow_mut().act(&candidate.root, "overwrite-c");
                    }
                    save(&candidate, tx)
                },
                &mut |edge| {
                    if action == "substitute" && edge == Edge::BeforeCapture {
                        worker.borrow_mut().act(&candidate.root, action);
                    }
                    if action != "substitute" && edge == Edge::BeforeUnlink {
                        candidate.assert_a(&candidate.target()); // witness EmuWiz publication BEFORE the external action
                        worker.borrow_mut().act(&candidate.root, action);
                    }
                    Ok(())
                },
                Fault::None,
            )
            .unwrap();
            let b_objects = objects(&baseline, None);
            let r_objects = objects(&candidate, Some(&tx));
            for (b_id, r_id) in [
                (baseline.a.ino, candidate.a.ino),
                (baseline.b.ino, candidate.b.ino),
                (b_c, r_c),
            ] {
                for (_, bytes, _) in b_objects.iter().filter(|(ino, _, _)| *ino == b_id) {
                    assert!(
                        r_objects
                            .iter()
                            .any(|(ino, other, _)| *ino == r_id && other == bytes),
                        "G3 survivor mismatch action={action} iteration={iteration} baseline={} candidate={}",
                        baseline.root.display(),
                        candidate.root.display()
                    );
                }
            }
            let phase = contract::evidence(&tx, 0).unwrap().unwrap().phase;
            assert_eq!(
                tx.state == TransactionState::RolledBack,
                action == "overwrite-b"
            );
            if action == "substitute" {
                assert_eq!(phase, Phase::ForeignHeld);
                assert!(!candidate.target().exists());
            }
            if (iteration - shard).is_multiple_of(1000) {
                println!(
                    "STRESS shard={shard} {action} iteration={iteration}/10000 elapsed={}s",
                    started.elapsed().as_secs()
                );
            }
        }
        println!("STRESS shard={shard} {action} 1250/1250 PASS");
    }
}

#[test]
fn retained_external_writable_fd_during_cleanup_matches_ordinary_baseline() {
    let mut external = External::new();
    let baseline = quiet("retained-writer-control-");
    let candidate = quiet("retained-writer-r2-");
    external.act(&baseline.root, "pin-old");
    external.act(&candidate.root, "pin-old");
    assert_eq!(ordinary(&baseline).state, TransactionState::RolledBack);
    external.act(&baseline.root, "write-dest");
    fs::create_dir(candidate.root.join("journals")).unwrap();
    let mut tx = enrolled_transaction(&candidate);
    contract::undo_entry(
        &candidate.root,
        &mut tx,
        0,
        &mut |tx| save(&candidate, tx),
        &mut |edge| {
            if edge == Edge::BeforeUnlink {
                external.act(&candidate.root, "write-dest");
            }
            Ok(())
        },
        Fault::None,
    )
    .unwrap();
    assert_eq!(fs::read(baseline.target()).unwrap(), b"XXXX");
    assert_eq!(fs::read(candidate.target()).unwrap(), b"XXXX");
    assert_eq!(
        fs::metadata(baseline.target()).unwrap().ino(),
        baseline.a.ino
    );
    assert_eq!(
        fs::metadata(candidate.target()).unwrap().ino(),
        candidate.a.ino
    );
    assert_eq!(
        contract::evidence(&tx, 0).unwrap().unwrap().phase,
        Phase::Incomplete
    );
    assert_ne!(tx.state, TransactionState::RolledBack);
}
