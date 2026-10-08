//! Process observations never waive an unresolved live holder.
use super::*;

#[derive(Clone, Debug)]
pub struct ProcScanReport {
    pub state: EmulatorQuiescence,
    pub card_holders: Vec<u32>,
    pub emulator_processes: Vec<u32>,
    pub unreadable_processes: Vec<u32>,
    pub listing_unavailable: bool,
}

pub struct ProcScanQuiescence {
    proc_root: PathBuf,
}
impl ProcScanQuiescence {
    pub fn new() -> Self {
        Self {
            proc_root: PathBuf::from("/proc"),
        }
    }
    pub fn with_root(proc_root: PathBuf) -> Self {
        Self { proc_root }
    }
    pub fn report(&self, binding: &DirectorySaveBinding) -> ProcScanReport {
        let mut report = ProcScanReport {
            state: EmulatorQuiescence::Unknown,
            card_holders: Vec::new(),
            emulator_processes: Vec::new(),
            unreadable_processes: Vec::new(),
            listing_unavailable: false,
        };
        let Ok(card) = fs::metadata(&binding.profile) else {
            report.listing_unavailable = true;
            return report;
        };
        let Ok(entries) = fs::read_dir(&self.proc_root) else {
            report.listing_unavailable = true;
            return report;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                report.listing_unavailable = true;
                continue;
            };
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            if pid == std::process::id() {
                continue;
            } // own read-only pins are not emulator holders
            let dir = entry.path();
            // Zombie processes have already closed their descriptor table. Exited
            // processes and these readable terminal states do not poison scans.
            if fs::read_to_string(dir.join("status")).is_ok_and(|status| {
                status.lines().any(|line| {
                    line.starts_with("State:") && line.split_whitespace().nth(1) == Some("Z")
                })
            }) {
                continue;
            }
            let mut unreadable = false;
            for leaf in ["comm", "cmdline"] {
                match fs::read(dir.join(leaf)) {
                    Ok(bytes) => {
                        let tokens: Vec<_> = bytes
                            .split(|byte| *byte == 0)
                            .filter_map(|bytes| std::str::from_utf8(bytes).ok())
                            .collect();
                        let executable = tokens.first().copied().unwrap_or("");
                        let launcher = Path::new(executable.trim())
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("");
                        if emulator_name(executable)
                            || (["flatpak", "bwrap"].contains(&launcher)
                                && tokens.iter().any(|token| {
                                    *token == "net.pcsx2.PCSX2" || emulator_name(token)
                                }))
                        {
                            report.emulator_processes.push(pid);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound && !dir.exists() => {
                    }
                    Err(_) => unreadable = true,
                }
            }
            if let Ok(executable) = fs::read_link(dir.join("exe"))
                && emulator_name(&executable.to_string_lossy())
            {
                report.emulator_processes.push(pid);
            }
            match fs::read_dir(dir.join("fd")) {
                Ok(fds) => {
                    for fd in fds {
                        match fd.and_then(|fd| fs::metadata(fd.path())) {
                            Ok(metadata)
                                if metadata.dev() == card.dev() && metadata.ino() == card.ino() =>
                            {
                                report.card_holders.push(pid)
                            }
                            Ok(_) => {}
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                            Err(_) => unreadable = true,
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && !dir.exists() => {}
                Err(_) => unreadable = true,
            }
            if unreadable && dir.exists() {
                report.unreadable_processes.push(pid);
            }
        }
        report.card_holders.sort_unstable();
        report.card_holders.dedup();
        report.emulator_processes.sort_unstable();
        report.emulator_processes.dedup();
        report.state = if !report.card_holders.is_empty() || !report.emulator_processes.is_empty() {
            EmulatorQuiescence::Running
        } else if report.listing_unavailable || !report.unreadable_processes.is_empty() {
            EmulatorQuiescence::Unknown
        } else {
            EmulatorQuiescence::Closed
        };
        report
    }
}
impl Default for ProcScanQuiescence {
    fn default() -> Self {
        Self::new()
    }
}
impl QuiescenceProvider for ProcScanQuiescence {
    fn observe(&self, binding: &DirectorySaveBinding) -> EmulatorQuiescence {
        self.report(binding).state
    }
}
fn emulator_name(value: &str) -> bool {
    let name = Path::new(value.trim())
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    PS2_EMULATOR_PROCESS_NAMES.iter().any(|expected| {
        name == *expected
            || name.strip_prefix(expected).is_some_and(|suffix| {
                suffix.starts_with('-') || suffix == ".appimage" || suffix == ".exe"
            })
    })
}
