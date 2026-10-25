use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizedProcessSpec {
    pub executable: PathBuf,
    pub home: PathBuf,
    pub config: PathBuf,
    pub data: PathBuf,
    pub current_dir: PathBuf,
    pub session_environment: BTreeMap<String, String>,
}

pub fn sanitized_command(spec: &SanitizedProcessSpec) -> Command {
    let mut command = Command::new(&spec.executable);
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C.UTF-8")
        .env("HOME", &spec.home)
        .env("XDG_CONFIG_HOME", &spec.config)
        .env("XDG_DATA_HOME", &spec.data)
        .envs(&spec.session_environment)
        .current_dir(&spec.current_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

pub fn bounded_read<R: Read>(reader: R, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > limit {
        return Err("process output exceeds its bound".into());
    }
    Ok(bytes)
}

pub fn wait_bounded(
    child: &mut Child,
    timeout: Duration,
) -> Result<std::process::ExitStatus, String> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait().map_err(|error| error.to_string())? {
            Some(status) => return Ok(status),
            None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("process timed out".into());
            }
        }
    }
}

pub fn validate_process_paths(spec: &SanitizedProcessSpec) -> Result<(), String> {
    for path in [
        &spec.executable,
        &spec.home,
        &spec.config,
        &spec.data,
        &spec.current_dir,
    ] {
        if !path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
        {
            return Err(format!("unsafe process path: {}", path.display()));
        }
    }
    Ok(())
}
