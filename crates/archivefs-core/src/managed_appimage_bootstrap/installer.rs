use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use sha2::Digest;

use crate::emulator_download::EmulatorDownloadTransport;

use super::model::{BootstrapError, EmulatorBootstrapPlan};
use super::safety::{file_hash, safe_path, validate_appimage_header};

pub fn revalidate_release(
    plan: &EmulatorBootstrapPlan,
    transport: &dyn EmulatorDownloadTransport,
) -> Result<(), BootstrapError> {
    let Some(reviewed) = &plan.download else {
        return Ok(());
    };
    let fresh = crate::emulator_download::resolve_download_plan(
        &plan.context.root,
        crate::emulator_download::emulator_download_spec(&reviewed.emulator_id)
            .ok_or_else(|| BootstrapError::Policy("unknown download specification".into()))?,
        transport,
        &crate::emulator_download::EmulatorDownloadOptions::default(),
    )
    .map_err(|error| BootstrapError::Policy(error.to_string()))?;
    if &fresh != reviewed {
        return Err(BootstrapError::StalePlan("release metadata or destination"));
    }
    Ok(())
}

pub fn publish_no_clobber(
    staged: &Path,
    destination: &Path,
    marker_staged: &Path,
    marker: &Path,
    expected_digest: &str,
    minimum_size: u64,
) -> Result<(), BootstrapError> {
    safe_path(staged)?;
    safe_path(destination)?;
    safe_path(marker_staged)?;
    safe_path(marker)?;
    if fs::symlink_metadata(destination).is_ok() || fs::symlink_metadata(marker).is_ok() {
        return Err(BootstrapError::Publication(
            "managed destination already exists".into(),
        ));
    }
    let actual = file_hash(staged, super::safety::MAX_HASH_BYTES)?
        .ok_or_else(|| BootstrapError::Publication("staged executable is missing".into()))?;
    if actual != expected_digest {
        return Err(BootstrapError::Publication("staged digest mismatch".into()));
    }
    validate_appimage_header(staged, minimum_size)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(staged, fs::Permissions::from_mode(0o755))
            .map_err(|error| BootstrapError::Publication(error.to_string()))?;
    }
    let parent = destination
        .parent()
        .ok_or_else(|| BootstrapError::Publication("destination has no parent".into()))?;
    fs::create_dir_all(parent).map_err(|error| BootstrapError::Publication(error.to_string()))?;
    fs::hard_link(staged, destination)
        .map_err(|error| BootstrapError::Publication(error.to_string()))?;
    if let Err(error) = fs::hard_link(marker_staged, marker) {
        return Err(BootstrapError::Publication(format!(
            "binary published but provenance marker publication failed: {error}"
        )));
    }
    let _ = fs::remove_file(staged);
    let _ = fs::remove_file(marker_staged);
    Ok(())
}

pub fn stream_to_file<R: Read>(
    mut reader: R,
    destination: &Path,
    maximum: u64,
) -> Result<String, BootstrapError> {
    safe_path(destination)?;
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(destination)
            .map_err(|error| BootstrapError::Publication(error.to_string()))?;
        let mut hash = sha2::Sha256::new();
        let mut total = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|error| BootstrapError::Publication(error.to_string()))?;
            if read == 0 {
                break;
            }
            total = total.saturating_add(read as u64);
            if total > maximum {
                return Err(BootstrapError::Publication(
                    "download exceeds its bound".into(),
                ));
            }
            file.write_all(&buffer[..read])
                .map_err(|error| BootstrapError::Publication(error.to_string()))?;
            hash.update(&buffer[..read]);
        }
        Ok(hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    })();
    if result.is_err() {
        let _ = fs::remove_file(destination);
    }
    result
}
