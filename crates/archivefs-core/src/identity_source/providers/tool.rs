use super::{MAX_SNAPSHOT_BYTES, ProviderResult};
use crate::dat::archive::external_process::{ProcessLimits, run_supervised};
use sha2::{Digest, Sha256};
use std::{ffi::OsString, fs::File, io::Read, path::Path, process::Command, time::Duration};

pub fn fingerprint(path: &Path) -> ProviderResult<String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > 512 * 1024 * 1024 {
        return Err("Executable is not a bounded regular file".into());
    }
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    let after = file.metadata().map_err(|e| e.to_string())?;
    if metadata.len() != after.len() || metadata.modified().ok() != after.modified().ok() {
        return Err("Executable changed during check".into());
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

pub fn run(
    executable: &Path,
    args: &[OsString],
    scummvm: bool,
    limit: u64,
) -> ProviderResult<(Vec<u8>, String)> {
    let mut output = Vec::new();
    let stderr = run_streaming(
        executable,
        args,
        scummvm,
        limit.min(MAX_SNAPSHOT_BYTES),
        |chunk| {
            output.extend_from_slice(chunk);
            Ok(())
        },
    )?;
    Ok((output, stderr))
}

/// Runs the tool with stdout streamed straight into `out` (never held in
/// memory), hashing it on the way. Returns `(sha256 of stdout, bytes written,
/// stderr)`. The supervisor refuses output beyond `limit`.
pub fn run_to_writer(
    executable: &Path,
    args: &[OsString],
    limit: u64,
    out: &mut impl std::io::Write,
) -> ProviderResult<(String, u64, String)> {
    let mut hash = Sha256::new();
    let mut written = 0_u64;
    let stderr = run_streaming(executable, args, false, limit, |chunk| {
        hash.update(chunk);
        written += chunk.len() as u64;
        out.write_all(chunk).map_err(|e| e.to_string())
    })?;
    let digest = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    Ok((digest, written, stderr))
}

fn run_streaming(
    executable: &Path,
    args: &[OsString],
    scummvm: bool,
    limit: u64,
    sink: impl FnMut(&[u8]) -> Result<(), String>,
) -> ProviderResult<String> {
    let scratch = tempfile::tempdir().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(executable);
    cmd.current_dir(scratch.path());
    if scummvm {
        cmd.arg(format!(
            "--config={}",
            scratch.path().join("isolated.ini").display()
        ));
        cmd.arg(format!(
            "--logfile={}",
            scratch.path().join("isolated.log").display()
        ));
    } else {
        cmd.arg("-noreadconfig");
    }
    cmd.args(args);
    let outcome = run_supervised(
        cmd,
        // MAME's full listxml needs more address space than a small probe (it
        // starts worker threads); the real 0.264 build fails at 2 GiB.
        ProcessLimits {
            address_space_bytes: if scummvm { 2 } else { 4 } * 1024 * 1024 * 1024,
            cpu_seconds: 60,
        },
        Duration::from_secs(60),
        limit,
        sink,
        None,
    )
    .map_err(|e| e.to_string())?;
    let stderr = String::from_utf8_lossy(&outcome.stderr).into_owned();
    if !outcome.status.success() {
        return Err(format!(
            "Provider tool failed: {}: {stderr}",
            outcome.status
        ));
    }
    Ok(stderr)
}
