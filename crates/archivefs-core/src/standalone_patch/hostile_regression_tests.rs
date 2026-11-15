//! Night Watch S08-02 / S08-03 regressions for the standalone patch
//! applier: hostile patch bodies must be refused before they can allocate
//! or write without bound, and the bytes applied must be the bytes that
//! were inspected.

use super::*;
use std::process::Command;

/// Spec BPS/UPS varint encoder (same as the in-module `tests::var`).
fn var(mut n: u64) -> Vec<u8> {
    let mut o = Vec::new();
    loop {
        let x = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            o.push(x | 0x80);
            return o;
        }
        o.push(x);
        n -= 1;
    }
}

fn bps(source: u64, target: u64, body: &[u8]) -> Vec<u8> {
    let mut b = b"BPS1".to_vec();
    b.extend(var(source));
    b.extend(var(target));
    b.push(0x80); // metadata length 0
    b.extend(body);
    b.extend([0u8; 8]); // source/target CRCs (not reached)
    let c = crc32(&b);
    b.extend(c.to_le_bytes());
    b
}

/// Re-runs one `#[ignore]` test of this binary in a child with a 256 MiB
/// address-space soft limit, so an unbounded allocation aborts the child
/// instead of the whole test run.
fn run_bounded_child(test_name: &str) -> std::process::ExitStatus {
    use std::os::unix::process::CommandExt;
    let mut c = Command::new(std::env::current_exe().unwrap());
    c.args([
        "--exact",
        test_name,
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]);
    // SAFETY: the closure only calls `getrlimit`/`setrlimit` on the child's
    // own limits before exec.
    unsafe {
        c.pre_exec(|| {
            let mut lim = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            libc::getrlimit(libc::RLIMIT_AS, &mut lim);
            lim.rlim_cur = 256 * 1024 * 1024;
            libc::setrlimit(libc::RLIMIT_AS, &lim);
            Ok(())
        });
    }
    c.status().unwrap()
}

#[test]
#[ignore = "child half of bps_target_copy_length_bomb_is_rejected_before_allocation"]
fn bps_target_copy_bomb_child() {
    // SourceRead 1 byte, then TargetCopy n = 2^40 from offset 0 (a
    // self-overlapping run that would grow the output byte by byte).
    let n: u64 = 1 << 40;
    let mut body = var(0); // a = ((1-1) << 2) | 0  -> SourceRead 1
    body.extend(var(((n - 1) << 2) | 3)); // TargetCopy n
    body.extend(var(0)); // relative offset +0
    let patch = bps(1, 16, &body);
    assert!(patch.len() < 40, "bomb is tiny: {} bytes", patch.len());
    let started = std::time::Instant::now();
    let result = apply_bps(&[0x41], &patch);
    assert!(result.is_err(), "length bomb must be rejected");
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

#[test]
fn bps_target_copy_length_bomb_is_rejected_before_allocation() {
    let status =
        run_bounded_child("standalone_patch::hostile_regression_tests::bps_target_copy_bomb_child");
    assert!(
        status.success(),
        "S08-02: apply_bps allocated without bound (child {status:?})"
    );
}

#[test]
fn bps_with_a_corrupt_checksum_is_refused_before_any_action_is_decoded() {
    let mut body = var(0); // SourceRead 1
    let mut patch = bps(1, 1, &body);
    let last = patch.len() - 1;
    patch[last] ^= 0xff;
    let error = apply_bps(&[0x41], &patch).unwrap_err().to_string();
    assert!(error.contains("CRC"), "{error}");
    body.clear();
}

#[test]
fn inspected_bytes_are_the_bytes_an_apply_decodes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("one.bps");
    let patch = bps(1, 1, &var(0));
    fs::write(&path, &patch).unwrap();
    let (inspection, bytes) = inspect_standalone_patch_bytes(&path).unwrap();
    assert_eq!(bytes, patch);
    assert_eq!(inspection.patch_sha256, hex_digest(&bytes));
    assert_eq!(inspection.format, StandalonePatchFormat::Bps);
}

#[test]
fn oversized_patch_file_is_refused_from_its_metadata_before_being_read() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("huge.ips");
    let file = fs::File::create(&path).unwrap();
    // Sparse: no bytes are written, so a refusal here proves the metadata
    // check ran before any read.
    file.set_len(MAX_PATCH_BYTES as u64 + 1).unwrap();
    let started = std::time::Instant::now();
    assert!(matches!(
        inspect_standalone_patch_bytes(&path),
        Err(StandalonePatchError::TooLarge)
    ));
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

/// S08-03: a decoded stage above the bounded apply limit is refused from
/// its metadata, never read into memory (the sparse file is never touched).
#[test]
fn oversized_xdelta_stage_is_refused_before_being_read() {
    let temp = tempfile::tempdir().unwrap();
    let stage = temp.path().join(".emuwiz-xdelta-stage-test");
    let file = fs::File::create(&stage).unwrap();
    file.set_len(MAX_APPLY_BYTES + 1).unwrap();
    let started = std::time::Instant::now();
    assert!(matches!(
        read_xdelta3_stage(&stage),
        Err(StandalonePatchError::TooLarge)
    ));
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    file.set_len(MAX_APPLY_BYTES).unwrap();
    assert!(read_xdelta3_stage(&stage).is_ok());
}

#[test]
fn xdelta_stage_that_is_a_symlink_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let real = temp.path().join("real.bin");
    fs::write(&real, b"decoded").unwrap();
    let stage = temp.path().join(".emuwiz-xdelta-stage-link");
    std::os::unix::fs::symlink(&real, &stage).unwrap();
    assert!(matches!(
        read_xdelta3_stage(&stage),
        Err(StandalonePatchError::UnsafeOutput(_))
    ));
}

/// One VCDIFF window with no source: RUN of `len` copies of 0x41.
fn vcdiff_run_window(len: u64) -> Vec<u8> {
    fn v(mut n: u64) -> Vec<u8> {
        // VCDIFF big-endian base-128 integer
        let mut tmp = vec![(n & 0x7f) as u8];
        n >>= 7;
        while n > 0 {
            tmp.push(((n & 0x7f) as u8) | 0x80);
            n >>= 7;
        }
        tmp.reverse();
        tmp
    }
    let data = [0x41u8];
    let mut inst = vec![0x00u8]; // default code table entry 0: RUN, size follows
    inst.extend(v(len));
    let mut enc = Vec::new();
    enc.extend(v(len)); // target window length
    enc.push(0); // delta indicator
    enc.extend(v(data.len() as u64));
    enc.extend(v(inst.len() as u64));
    enc.extend(v(0)); // addresses
    enc.extend(data);
    enc.extend(inst);
    let mut w = vec![0u8]; // win indicator: no source/target segment
    w.extend(v(enc.len() as u64));
    w.extend(enc);
    w
}

/// S08-03 end to end with a real `xdelta3`: a tiny patch that would decode
/// to more than the bounded apply limit fails closed in the child (the
/// kernel's file-size limit) and leaves no stage residue. Writes up to the
/// limit to disk, so it only runs when `EMUWIZ_HEAVY_PATCH_TESTS=1` and an
/// `xdelta3` is on `PATH`.
#[test]
fn xdelta3_output_is_bounded_before_emuwiz_reads_the_stage() {
    if std::env::var_os("EMUWIZ_HEAVY_PATCH_TESTS").is_none() || locate_xdelta3().is_none() {
        eprintln!("skipping: needs EMUWIZ_HEAVY_PATCH_TESTS=1 and an xdelta3 on PATH");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("base.bin");
    fs::write(&base, b"base").unwrap();
    let window = 16 * 1024 * 1024; // xdelta3 hard max window
    let windows = (MAX_APPLY_BYTES / window) + 2;
    let mut patch = vec![0xd6, 0xc3, 0xc4, 0x00, 0x00];
    for _ in 0..windows {
        patch.extend(vcdiff_run_window(window));
    }
    assert!(patch.len() < 4096, "amplification patch is small");
    let patch_path = temp.path().join("bomb.xdelta");
    fs::write(&patch_path, &patch).unwrap();
    let result = apply_xdelta3(&base, &patch_path, temp.path(), &patch);
    assert!(
        result.is_err(),
        "S08-03: output above MAX_APPLY_BYTES must fail closed"
    );
    let residue: Vec<_> = fs::read_dir(temp.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(".emuwiz-xdelta-stage")
        })
        .collect();
    assert!(
        residue.is_empty(),
        "S08-03: stage residue left beside the base"
    );
}
