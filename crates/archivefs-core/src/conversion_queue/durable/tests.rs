use super::*;
use crate::wiiu_conversion::{
    WiiUConversionDirection, WiiUConversionIdentity, WiiUConversionRequest,
    WiiUConversionToolInventory, plan_wiiu_conversion,
};
use crate::wiiu_disc::{WUD_HEADER_SIZE, WUD_SECTOR_SIZE, inspect_wii_u_disc};
use std::io::{Seek, SeekFrom, Write};
use tempfile::{TempDir, tempdir};

// Synthetic sector map with repeated zero blocks; the real decoder, independent
// output hash, header verification and Repair publication all run in these tests.
fn fixture(path: &Path) -> Vec<u8> {
    let sector = WUD_SECTOR_SIZE as usize;
    let mut raw = vec![0; 8 * sector];
    raw[..10].copy_from_slice(b"WUP-P-TEST");
    raw[0x10000..0x10004].copy_from_slice(&0xcc54_9eb9_u32.to_be_bytes());
    raw[0x10005] = 1;
    raw[0x18000..WUD_HEADER_SIZE as usize].fill(0xa5);
    let mut data = vec![0; 5 * sector];
    data[..4].copy_from_slice(b"WUX0");
    data[4..8].copy_from_slice(&0x1099_d02e_u32.to_le_bytes());
    data[8..12].copy_from_slice(&WUD_SECTOR_SIZE.to_le_bytes());
    data[16..24].copy_from_slice(&(raw.len() as u64).to_le_bytes());
    for (i, block) in [0_u32, 1, 2, 3, 1, 1, 1, 1].into_iter().enumerate() {
        data[32 + i * 4..36 + i * 4].copy_from_slice(&block.to_le_bytes());
    }
    data[sector..].copy_from_slice(&raw[..4 * sector]);
    fs::write(path, data).unwrap();
    raw
}

fn reviewed(root: &Path, name: &str, destination: &str) -> ReviewedConversion {
    let source = root.join(name);
    if !source.exists() {
        fixture(&source);
    }
    ReviewedConversion::WuxToWud(Box::new(plan_wiiu_conversion(&WiiUConversionRequest {
        source,
        destination: root.join(destination),
        direction: WiiUConversionDirection::WuxToWud,
        source_identity: WiiUConversionIdentity::HashMissing,
        available_free_space: Some(u64::MAX),
        tools: WiiUConversionToolInventory::default(),
    })))
}

fn setup() -> (TempDir, DurableConversionQueue, u64) {
    let dir = tempdir().unwrap();
    let mut queue = DurableConversionQueue::open(&dir.path().join("queue")).unwrap();
    let id = queue
        .enqueue(reviewed(dir.path(), "in.wux", "out.wud"))
        .unwrap();
    (dir, queue, id)
}

fn run(queue: &mut DurableConversionQueue) -> ConversionJob {
    queue
        .run_next(&AtomicBool::new(false), &mut |_| {})
        .unwrap()
        .unwrap()
}

fn edit_source(dir: &Path) {
    let file = dir.join("in.wux");
    let mut data = fs::read(&file).unwrap();
    *data.last_mut().unwrap() ^= 1;
    fs::write(file, data).unwrap();
}

fn mark_running(queue: &mut DurableConversionQueue) {
    let staging = tempfile::Builder::new()
        .prefix(".emuwiz-conversion-queue-")
        .tempdir_in(queue.root.parent().unwrap())
        .unwrap()
        .keep();
    let journal_dir = queue.root.join("transactions/1/1");
    safe_directory(&journal_dir, true).unwrap();
    let mut next = queue.snapshot.clone();
    next.jobs[0].state = JobState::Running;
    next.jobs[0].attempts.push(ConversionAttempt {
        journal_dir,
        staging_root: staging,
        started_at_unix: now_unix(),
        state: JobState::Running,
    });
    next.jobs[0].progress = Some(ProgressSnapshot {
        bytes_processed: Some(123),
        total_bytes: Some(262144),
        phase: "converting".into(),
        updated_at_unix: now_unix(),
    });
    queue.commit(next).unwrap();
}

#[test]
fn enqueue_binds_original_plan_identity_and_time() {
    let (_dir, queue, id) = setup();
    let job = &queue.snapshot.jobs[0];
    assert_eq!((id, job.order, job.state), (1, 1, JobState::Queued));
    assert!(job.created_at_unix > 0);
    assert!(job.source_identity.freshness.is_some());
    assert_eq!(
        DurableConversionQueue::inspect(&queue.root).unwrap(),
        queue.snapshot
    );
}

#[test]
fn several_jobs_run_in_fifo_order_with_stable_ids() {
    let (dir, mut queue, first) = setup();
    let second = queue
        .enqueue(reviewed(dir.path(), "in.wux", "two.wud"))
        .unwrap();
    let third = queue
        .enqueue(reviewed(dir.path(), "in.wux", "three.wud"))
        .unwrap();
    for id in [first, second, third] {
        assert_eq!(run(&mut queue).id, id);
    }
    assert!(
        queue
            .run_next(&AtomicBool::new(false), &mut |_| {})
            .unwrap()
            .is_none()
    );
}

#[test]
fn restart_preserves_queued_jobs_and_sequence() {
    let (dir, queue, id) = setup();
    let before = queue.snapshot.clone();
    drop(queue);
    let mut queue = DurableConversionQueue::open(&dir.path().join("queue")).unwrap();
    assert_eq!(queue.snapshot, before);
    assert_eq!(
        queue
            .enqueue(reviewed(dir.path(), "in.wux", "two.wud"))
            .unwrap(),
        id + 1
    );
}

#[test]
fn changed_queued_source_is_blocked_without_output() {
    let (dir, mut queue, _) = setup();
    edit_source(dir.path());
    assert_eq!(run(&mut queue).state, JobState::BlockedStale);
    assert!(!dir.path().join("out.wud").exists());
}

#[test]
fn missing_source_has_distinct_blocked_state() {
    let (dir, mut queue, _) = setup();
    fs::rename(dir.path().join("in.wux"), dir.path().join("retained.wux")).unwrap();
    assert_eq!(run(&mut queue).state, JobState::BlockedInputMissing);
}

#[test]
fn destination_appearing_after_enqueue_is_never_overwritten() {
    let (dir, mut queue, _) = setup();
    fs::write(dir.path().join("out.wud"), b"existing destination").unwrap();
    assert_eq!(run(&mut queue).state, JobState::BlockedStale);
    assert_eq!(
        fs::read(dir.path().join("out.wud")).unwrap(),
        b"existing destination"
    );
}

#[test]
fn duplicate_destination_is_refused_even_for_shared_source() {
    let (dir, mut queue, _) = setup();
    assert!(
        queue
            .enqueue(reviewed(dir.path(), "in.wux", "out.wud"))
            .is_err()
    );
    assert_eq!(queue.snapshot.jobs.len(), 1);
}

#[test]
fn real_wux_decode_verifies_and_persists_completed_result_across_restart() {
    use sha2::{Digest, Sha256};
    let (dir, mut queue, _) = setup();
    let source = fs::read(dir.path().join("in.wux")).unwrap();
    let expected = fixture(&dir.path().join("reference.wux"));
    let job = run(&mut queue);
    assert_eq!(job.state, JobState::Completed);
    let record = job.result.as_ref().unwrap();
    assert_eq!(
        record.output_sha256,
        wiiu_conversion::digest_hex(Sha256::digest(&expected))
    );
    assert_eq!(record.output_sha256, record.reconstructed_wud_sha256);
    assert_eq!(fs::read(dir.path().join("out.wud")).unwrap(), expected);
    assert!(inspect_wii_u_disc(&dir.path().join("out.wud")).structural_complete);
    assert_eq!(fs::read(dir.path().join("in.wux")).unwrap(), source);
    let (journals, problems) =
        crate::dat::rename_apply::journal::list_journals(&job.attempts[0].journal_dir);
    assert!(problems.is_empty());
    assert_eq!(journals.len(), 1);
    assert_eq!(journals[0].transaction_id, record.transaction_id);
    assert_eq!(
        journals[0].state,
        crate::dat::rename_apply::model::TransactionState::Applied
    );
    drop(queue);
    let reopened = DurableConversionQueue::open(&dir.path().join("queue")).unwrap();
    assert_eq!(reopened.snapshot.jobs[0], job);
}

fn fail_verification(queue: &mut DurableConversionQueue) -> ConversionJob {
    let root = queue.root.clone();
    queue
        .run_next(&AtomicBool::new(false), &mut |p| {
            if p.bytes_processed == p.total_bytes {
                let snapshot = DurableConversionQueue::inspect(&root).unwrap();
                let staging = &snapshot.jobs[0].attempts.last().unwrap().staging_root;
                let child = fs::read_dir(staging)
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path();
                let mut output = fs::OpenOptions::new()
                    .write(true)
                    .open(child.join("output.wud"))
                    .unwrap();
                output.seek(SeekFrom::Start(0)).unwrap();
                output.write_all(b"damage").unwrap();
            }
        })
        .unwrap()
        .unwrap()
}

#[test]
fn failed_verification_persists_failed_and_never_publishes_partial_output() {
    let (dir, mut queue, _) = setup();
    let before = fs::read(dir.path().join("in.wux")).unwrap();
    assert_eq!(fail_verification(&mut queue).state, JobState::Failed);
    assert!(!dir.path().join("out.wud").exists());
    assert_eq!(fs::read(dir.path().join("in.wux")).unwrap(), before);
    drop(queue);
    assert_eq!(
        DurableConversionQueue::open(&dir.path().join("queue"))
            .unwrap()
            .snapshot
            .jobs[0]
            .state,
        JobState::Failed
    );
}

#[test]
fn failure_can_retry_original_job_at_fifo_tail() {
    let (dir, mut queue, first) = setup();
    fail_verification(&mut queue);
    let second = queue
        .enqueue(reviewed(dir.path(), "in.wux", "second.wud"))
        .unwrap();
    assert_eq!(queue.retry(first).unwrap(), JobState::Queued);
    assert_eq!(run(&mut queue).id, second);
    let retried = run(&mut queue);
    assert_eq!(
        (retried.id, retried.state, retried.attempts.len()),
        (first, JobState::Completed, 2)
    );
}

#[test]
fn queued_cancel_is_durable_and_does_not_touch_source_or_destination() {
    let (dir, mut queue, id) = setup();
    let before = fs::read(dir.path().join("in.wux")).unwrap();
    queue.cancel_queued(id).unwrap();
    assert_eq!(
        DurableConversionQueue::inspect(&queue.root).unwrap().jobs[0].state,
        JobState::Cancelled
    );
    assert!(
        queue
            .run_next(&AtomicBool::new(false), &mut |_| {})
            .unwrap()
            .is_none()
    );
    assert_eq!(fs::read(dir.path().join("in.wux")).unwrap(), before);
    assert!(!dir.path().join("out.wud").exists());
}

#[test]
fn cooperative_running_cancellation_never_publishes() {
    let (dir, mut queue, _) = setup();
    let before = fs::read(dir.path().join("in.wux")).unwrap();
    let cancel = AtomicBool::new(false);
    let job = queue
        .run_next(&cancel, &mut |_| cancel.store(true, Ordering::Relaxed))
        .unwrap()
        .unwrap();
    assert_eq!(job.state, JobState::Cancelled);
    assert_eq!(
        DurableConversionQueue::inspect(&queue.root).unwrap().jobs[0].state,
        JobState::Cancelled
    );
    assert!(!dir.path().join("out.wud").exists());
    assert_eq!(fs::read(dir.path().join("in.wux")).unwrap(), before);
    assert_eq!(
        fs::read_dir(&job.attempts[0].staging_root).unwrap().count(),
        0
    );
}

#[test]
fn restart_marks_running_interrupted_and_progress_does_not_authorize_resume() {
    let (dir, mut queue, id) = setup();
    mark_running(&mut queue);
    assert_eq!(
        DurableConversionQueue::inspect(&queue.root).unwrap().jobs[0].state,
        JobState::Running
    );
    drop(queue);
    let mut queue = DurableConversionQueue::open(&dir.path().join("queue")).unwrap();
    assert_eq!(queue.snapshot.jobs[0].state, JobState::Interrupted);
    assert_eq!(
        queue.snapshot.jobs[0].retry,
        Some(RetryDisposition::RestartFromBeginning)
    );
    assert!(
        queue.snapshot.jobs[0]
            .progress
            .as_ref()
            .unwrap()
            .phase
            .contains("historical")
    );
    assert_eq!(queue.retry(id).unwrap(), JobState::Queued);
    assert!(queue.snapshot.jobs[0].progress.is_none());
    assert_eq!(run(&mut queue).state, JobState::Completed);
}

#[test]
fn interrupted_retry_revalidates_original_source() {
    let (dir, mut queue, id) = setup();
    mark_running(&mut queue);
    drop(queue);
    edit_source(dir.path());
    let mut queue = DurableConversionQueue::open(&dir.path().join("queue")).unwrap();
    assert_eq!(queue.retry(id).unwrap(), JobState::BlockedStale);
    assert!(!dir.path().join("out.wud").exists());
}

#[test]
fn corrupt_and_truncated_states_are_preserved_and_refused() {
    for data in [
        b"not json".as_slice(),
        b"{\"schema_version\":1,\"jobs\":[".as_slice(),
    ] {
        let (dir, queue, _) = setup();
        let root = queue.root.clone();
        drop(queue);
        fs::write(root.join(SNAPSHOT_FILE), data).unwrap();
        assert!(DurableConversionQueue::inspect(&root).is_err());
        assert!(DurableConversionQueue::open(&root).is_err());
        assert_eq!(fs::read(root.join(SNAPSHOT_FILE)).unwrap(), data);
        assert!(dir.path().join("in.wux").exists());
    }
}

#[test]
fn unsupported_version_and_duplicate_ids_fail_closed() {
    let (_dir, queue, _) = setup();
    let mut next = queue.snapshot.clone();
    next.schema_version += 1;
    assert!(validate_snapshot(&queue.root, &next).is_err());
    next = queue.snapshot.clone();
    next.jobs.push(next.jobs[0].clone());
    assert!(validate_snapshot(&queue.root, &next).is_err());
}

#[test]
fn inspection_does_not_create_files_or_recover_running_state() {
    let dir = tempdir().unwrap();
    let missing = dir.path().join("missing");
    assert!(
        DurableConversionQueue::inspect(&missing)
            .unwrap()
            .jobs
            .is_empty()
    );
    assert!(!missing.exists());
    let (_dir, mut queue, _) = setup();
    mark_running(&mut queue);
    let path = queue.root.join(SNAPSHOT_FILE);
    let before = fs::read(&path).unwrap();
    let metadata = fs::metadata(&path).unwrap();
    let listing: Vec<_> = fs::read_dir(&queue.root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(
        DurableConversionQueue::inspect(&queue.root).unwrap().jobs[0].state,
        JobState::Running
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(
        fs::metadata(&path).unwrap().modified().unwrap(),
        metadata.modified().unwrap()
    );
    assert_eq!(fs::read_dir(&queue.root).unwrap().count(), listing.len());
}

#[test]
fn progress_is_bounded_checkpointed_and_live() {
    let (_dir, mut queue, _) = setup();
    let root = queue.root.clone();
    let mut updates = 0;
    queue
        .run_next(&AtomicBool::new(false), &mut |p| {
            updates += 1;
            assert!(p.bytes_processed.unwrap() <= p.total_bytes.unwrap());
            let saved = DurableConversionQueue::inspect(&root).unwrap();
            assert_eq!(saved.jobs[0].state, JobState::Running);
            assert!(
                saved.jobs[0]
                    .progress
                    .as_ref()
                    .unwrap()
                    .bytes_processed
                    .is_some()
            );
        })
        .unwrap();
    assert!(updates > 1);
}

#[test]
fn live_owner_cannot_be_misclassified_as_dead() {
    let (_dir, queue, _) = setup();
    assert!(DurableConversionQueue::open(&queue.root).is_err());
    assert_eq!(
        DurableConversionQueue::inspect(&queue.root).unwrap(),
        queue.snapshot
    );
}

#[test]
fn dropping_owner_unlocks_even_while_an_inherited_descriptor_remains_open() {
    let (_dir, queue, _) = setup();
    let root = queue.root.clone();
    // try_clone shares the open file description, just like fork inheritance.
    let inherited = queue.lock.try_clone().unwrap();
    drop(queue);
    let reopened = DurableConversionQueue::open(&root).unwrap();
    assert_eq!(reopened.snapshot.jobs[0].state, JobState::Queued);
    drop(inherited);
}

#[test]
fn publication_evidence_requires_review_and_is_retained() {
    let (dir, mut queue, id) = setup();
    mark_running(&mut queue);
    let journal = queue.snapshot.jobs[0].attempts[0]
        .journal_dir
        .join("broken.json");
    fs::write(&journal, b"partial publication journal").unwrap();
    drop(queue);
    let mut queue = DurableConversionQueue::open(&dir.path().join("queue")).unwrap();
    assert_eq!(queue.snapshot.jobs[0].state, JobState::Interrupted);
    assert_eq!(
        queue.snapshot.jobs[0].retry,
        Some(RetryDisposition::RequiresReview)
    );
    assert!(queue.retry(id).is_err());
    assert_eq!(fs::read(&journal).unwrap(), b"partial publication journal");
}

#[test]
fn pruning_is_explicit_retains_recovery_and_does_not_recycle_ids() {
    let (dir, mut queue, first) = setup();
    run(&mut queue);
    let second = queue
        .enqueue(reviewed(dir.path(), "in.wux", "second.wud"))
        .unwrap();
    queue.cancel_queued(second).unwrap();
    assert_eq!(queue.prune_completed(1).unwrap(), 1);
    assert_eq!(queue.snapshot.jobs[0].id, second);
    let third = queue
        .enqueue(reviewed(dir.path(), "in.wux", "third.wud"))
        .unwrap();
    assert!(third > second && second > first);
    assert_eq!(queue.prune_completed(0).unwrap(), 1);
    assert_eq!(queue.snapshot.jobs[0].state, JobState::Queued);
}

#[test]
fn oversized_state_is_refused_without_reading_or_truncating_it() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(SNAPSHOT_FILE);
    File::create(&path)
        .unwrap()
        .set_len(MAX_QUEUE_BYTES + 1)
        .unwrap();
    assert!(DurableConversionQueue::inspect(dir.path()).is_err());
    assert_eq!(fs::metadata(path).unwrap().len(), MAX_QUEUE_BYTES + 1);
}

#[cfg(unix)]
#[test]
fn symlinked_queue_and_destination_are_refused() {
    let (dir, mut queue, _) = setup();
    std::os::unix::fs::symlink(dir.path().join("missing"), dir.path().join("out.wud")).unwrap();
    assert_eq!(run(&mut queue).state, JobState::BlockedStale);
    let link = dir.path().join("queue-link");
    std::os::unix::fs::symlink(&queue.root, &link).unwrap();
    assert!(DurableConversionQueue::inspect(&link).is_err());
    assert!(DurableConversionQueue::open(&link).is_err());
}

#[test]
fn crash_child() {
    let Some(root) = std::env::var_os("EMUWIZ_QUEUE_CRASH_TEST_ROOT") else {
        return;
    };
    let mut queue = DurableConversionQueue::open(Path::new(&root)).unwrap();
    queue
        .run_next(&AtomicBool::new(false), &mut |_| std::process::exit(86))
        .unwrap();
    panic!("converter never reported progress");
}

#[test]
fn actual_process_exit_releases_lock_preserves_partial_stage_and_restarts_from_zero() {
    let (dir, queue, id) = setup();
    let root = queue.root.clone();
    let source = fs::read(dir.path().join("in.wux")).unwrap();
    drop(queue);
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "conversion_queue::durable::tests::crash_child",
            "--nocapture",
        ])
        .env("EMUWIZ_QUEUE_CRASH_TEST_ROOT", &root)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(86));
    assert_eq!(
        DurableConversionQueue::inspect(&root).unwrap().jobs[0].state,
        JobState::Running
    );
    assert!(!dir.path().join("out.wud").exists());
    let mut queue = DurableConversionQueue::open(&root).unwrap();
    assert_eq!(queue.snapshot.jobs[0].state, JobState::Interrupted);
    let abandoned = queue.snapshot.jobs[0].attempts[0].staging_root.clone();
    assert_eq!(fs::read_dir(&abandoned).unwrap().count(), 1);
    assert_eq!(queue.retry(id).unwrap(), JobState::Queued);
    let completed = run(&mut queue);
    assert_eq!(completed.state, JobState::Completed);
    assert_ne!(completed.attempts[1].staging_root, abandoned);
    assert!(abandoned.exists());
    assert_eq!(fs::read(dir.path().join("in.wux")).unwrap(), source);
}

#[test]
fn destination_created_during_decode_is_preserved() {
    let (dir, mut queue, _) = setup();
    let destination = dir.path().join("out.wud");
    let job = queue
        .run_next(&AtomicBool::new(false), &mut |_| {
            if !destination.exists() {
                fs::write(&destination, b"racing writer").unwrap();
            }
        })
        .unwrap()
        .unwrap();
    assert_eq!(job.state, JobState::BlockedStale);
    assert_eq!(fs::read(destination).unwrap(), b"racing writer");
}

#[test]
fn crash_after_publication_requires_review_then_confirmed_rollback_permits_retry() {
    let (dir, mut queue, id) = setup();
    let completed = run(&mut queue);
    let mut interrupted = queue.snapshot.clone();
    interrupted.jobs[0].state = JobState::Running;
    interrupted.jobs[0].attempts[0].state = JobState::Running;
    interrupted.jobs[0].result = None;
    queue.commit(interrupted).unwrap();
    drop(queue);
    let mut queue = DurableConversionQueue::open(&dir.path().join("queue")).unwrap();
    assert_eq!(queue.snapshot.jobs[0].state, JobState::Interrupted);
    assert_eq!(
        queue.snapshot.jobs[0].retry,
        Some(RetryDisposition::RequiresReview)
    );
    assert!(queue.retry(id).is_err());
    let journal_dir = &completed.attempts[0].journal_dir;
    let (mut transactions, problems) =
        crate::dat::rename_apply::journal::list_journals(journal_dir);
    assert!(problems.is_empty());
    crate::repair::execute::rollback_repair_transaction(
        &mut transactions[0],
        journal_dir,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(!dir.path().join("out.wud").exists());
    assert_eq!(queue.retry(id).unwrap(), JobState::Queued);
    assert_eq!(run(&mut queue).state, JobState::Completed);
}

#[test]
fn corrupt_state_during_owned_session_is_not_overwritten() {
    let (_dir, mut queue, id) = setup();
    fs::write(queue.root.join(SNAPSHOT_FILE), b"damaged during session").unwrap();
    assert!(queue.cancel_queued(id).is_err());
    assert!(queue.poisoned);
    assert_eq!(
        fs::read(queue.root.join(SNAPSHOT_FILE)).unwrap(),
        b"damaged during session"
    );
}

#[test]
fn missing_snapshot_with_recovery_evidence_is_not_reset() {
    let (_dir, mut queue, _) = setup();
    mark_running(&mut queue);
    let root = queue.root.clone();
    drop(queue);
    fs::remove_file(root.join(SNAPSHOT_FILE)).unwrap();
    assert!(DurableConversionQueue::inspect(&root).is_err());
    assert!(DurableConversionQueue::open(&root).is_err());
    assert!(!root.join(SNAPSHOT_FILE).exists());
}

#[test]
fn failed_attempt_evidence_survives_retry_cancel_and_pruning() {
    let (_dir, mut queue, id) = setup();
    fail_verification(&mut queue);
    assert_eq!(queue.snapshot.jobs[0].attempts[0].state, JobState::Failed);
    queue.retry(id).unwrap();
    queue.cancel_queued(id).unwrap();
    assert_eq!(queue.prune_completed(0).unwrap(), 0);
    assert_eq!(queue.snapshot.jobs[0].attempts[0].state, JobState::Failed);
}

#[test]
fn failure_preparing_attempt_is_persisted_and_can_retry_after_repair() {
    let (_dir, mut queue, id) = setup();
    let path = queue.root.join("transactions");
    fs::write(&path, b"not a directory").unwrap();
    assert_eq!(run(&mut queue).state, JobState::Failed);
    assert_eq!(
        DurableConversionQueue::inspect(&queue.root).unwrap().jobs[0].state,
        JobState::Failed
    );
    fs::remove_file(path).unwrap();
    assert_eq!(queue.retry(id).unwrap(), JobState::Queued);
    assert_eq!(run(&mut queue).state, JobState::Completed);
}

#[test]
fn queued_retry_rechecks_prior_publication_evidence_before_starting() {
    let (dir, mut queue, id) = setup();
    fail_verification(&mut queue);
    queue.retry(id).unwrap();
    let journal = queue.snapshot.jobs[0].attempts[0]
        .journal_dir
        .join("new.json");
    fs::write(journal, b"unknown publication evidence").unwrap();
    let job = run(&mut queue);
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(job.retry, Some(RetryDisposition::RequiresReview));
    assert_eq!(job.attempts.len(), 1);
    assert!(!dir.path().join("out.wud").exists());
}

#[test]
fn stale_review_is_refused_at_enqueue_and_retry_checks_again_before_execution() {
    let dir = tempdir().unwrap();
    let mut queue = DurableConversionQueue::open(&dir.path().join("queue")).unwrap();
    let plan = reviewed(dir.path(), "in.wux", "out.wud");
    edit_source(dir.path());
    assert!(queue.enqueue(plan).is_err());
    let id = queue
        .enqueue(reviewed(dir.path(), "in.wux", "out.wud"))
        .unwrap();
    fail_verification(&mut queue);
    queue.retry(id).unwrap();
    edit_source(dir.path());
    assert_eq!(run(&mut queue).state, JobState::BlockedStale);
    assert!(!dir.path().join("out.wud").exists());
}

#[test]
fn replacing_locked_directory_refuses_to_write_to_new_queue() {
    let (dir, mut queue, id) = setup();
    fs::rename(&queue.root, dir.path().join("old-queue")).unwrap();
    fs::create_dir(&queue.root).unwrap();
    assert!(queue.cancel_queued(id).is_err());
    assert_eq!(fs::read_dir(&queue.root).unwrap().count(), 0);
}

#[test]
fn progress_checkpoint_failure_stops_before_publication_and_requires_recovery() {
    let (dir, mut queue, _) = setup();
    let snapshot = queue.root.join(SNAPSHOT_FILE);
    let cancel = AtomicBool::new(false);
    let mut damaged = false;
    let result = queue.run_next(&cancel, &mut |_| {
        if !damaged {
            damaged = true;
            fs::write(&snapshot, b"corrupt").unwrap();
            // The next chunk reaches the real one-second persistence throttle.
            std::thread::sleep(Duration::from_millis(1050));
        }
    });
    assert!(result.is_err());
    assert!(queue.poisoned);
    assert!(cancel.load(Ordering::Relaxed));
    assert_eq!(fs::read(snapshot).unwrap(), b"corrupt");
    assert!(!dir.path().join("out.wud").exists());
}
