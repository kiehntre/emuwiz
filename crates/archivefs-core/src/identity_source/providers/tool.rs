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
    let mut output = Vec::new();
    let outcome = run_supervised(
        cmd,
        ProcessLimits {
            address_space_bytes: 2 * 1024 * 1024 * 1024,
            cpu_seconds: 60,
        },
        Duration::from_secs(60),
        limit.min(MAX_SNAPSHOT_BYTES),
        |chunk| {
            output.extend_from_slice(chunk);
            Ok(())
        },
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
    Ok((output, stderr))
}
