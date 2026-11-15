//! Process observations never waive an unresolved live holder.
//!
//! The scan answers "is a PS2 emulator running, or visibly holding the
//! card?" from the processes it can read: executable, command line,
//! Flatpak/AppImage launch routes, Flatpak app scope, and descriptor tables.
//! Any such evidence reports `Running`. Names are only ever used to refuse,
//! never to clear a process.
//!
//! A process whose descriptor table cannot be read (another user, root, a
//! container, a non-dumpable process) makes the result `Unknown`. The one
//! exception is a scan made while the caller holds a kernel lease on the card
//! (`observe_under_card_lease`): the lease was granted only while no other
//! descriptor at all was open on the card (readers included), and it breaks
//! on any later open for writing. So the question that process's descriptors
//! would answer is answered by the kernel for every process instead, and the
//! process is reported as covered by the lease. A read-only open made after
//! the lease was granted is not seen; it cannot change the card, and a later
//! open for writing breaks the lease. That says nothing about what
//! the process is. It is only sound because the card operations hold the
//! lease across their whole publication window and reverse the exchange if
//! it breaks (see `card_lease` and `restore_guard`). A broken lease reports
//! `Running`. A process whose identity cannot be read stays `Unknown` even
//! under a lease, because the emulator check cannot be run on it.
use super::*;

#[derive(Clone, Debug)]
pub struct ProcScanReport {
    pub state: EmulatorQuiescence,
    pub card_holders: Vec<u32>,
    pub emulator_processes: Vec<u32>,
    /// Processes that could not be inspected and block (`Unknown`).
    pub unreadable_processes: Vec<u32>,
    pub listing_unavailable: bool,
    /// The caller's card lease broke during the scan: some process (possibly
    /// one we cannot inspect) opened the card for writing.
    pub card_open_for_writing: bool,
    /// Processes whose descriptors could not be read, whose part of the
    /// question was answered by the caller's intact card lease instead. Not
    /// a statement about what they are.
    pub covered_by_lease: Vec<u32>,
}

pub struct ProcScanQuiescence {
    proc_root: PathBuf,
}
impl ProcScanQuiescence {
    pub fn new() -> Self {
        Self::with_root(PathBuf::from("/proc"))
    }
    pub fn with_root(proc_root: PathBuf) -> Self {
        Self { proc_root }
    }
    /// Strict scan: every uninspectable process is `Unknown`.
    pub fn report(&self, binding: &DirectorySaveBinding) -> ProcScanReport {
        self.scan(binding, None)
    }
    /// Scan while the caller holds `lease` on the card (see module docs).
    pub fn report_under_card_lease(
        &self,
        binding: &DirectorySaveBinding,
        lease: &CardWriteLease,
    ) -> ProcScanReport {
        self.scan(binding, Some(lease))
    }
    fn scan(
        &self,
        binding: &DirectorySaveBinding,
        lease: Option<&CardWriteLease>,
    ) -> ProcScanReport {
        let who = EmulatorIdentity::for_binding(binding);
        let mut report = ProcScanReport {
            state: EmulatorQuiescence::Unknown,
            card_holders: Vec::new(),
            emulator_processes: Vec::new(),
            unreadable_processes: Vec::new(),
            listing_unavailable: false,
            card_open_for_writing: false,
            covered_by_lease: Vec::new(),
        };
        let Ok(card) = fs::metadata(&binding.profile) else {
            report.listing_unavailable = true;
            return report;
        };
        // A lease on some other inode proves nothing about this card.
        let lease = lease.filter(|lease| lease.inode() == Some((card.dev(), card.ino())));
        let Ok(entries) = fs::read_dir(&self.proc_root) else {
            report.listing_unavailable = true;
            return report;
        };
        // Uninspectable processes whose identity is readable.
        let mut uninspectable = Vec::new();
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
            let mut identity_unreadable = false;
            let mut cgroup_unreadable = false;
            let mut descriptors_unreadable = false;
            let mut tokens_seen: Vec<String> = Vec::new();
            let mut cgroup_text = String::new();
            for leaf in ["comm", "cmdline"] {
                match fs::read(dir.join(leaf)) {
                    Ok(bytes) => {
                        // Lossy, never dropped: a non-UTF-8 byte in a path must
                        // not hide the emulator's name from the match.
                        let tokens: Vec<String> = bytes
                            .split(|byte| *byte == 0)
                            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                            .collect();
                        let executable = tokens.first().map(String::as_str).unwrap_or("");
                        let launcher = Path::new(executable.trim())
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("");
                        if who.name(executable)
                            || (["flatpak", "bwrap"].contains(&launcher)
                                && tokens
                                    .iter()
                                    .any(|token| token.contains(who.flatpak_id) || who.name(token)))
                        {
                            report.emulator_processes.push(pid);
                        }
                        tokens_seen.extend(tokens.iter().map(|token| token.to_string()));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound && !dir.exists() => {
                    }
                    Err(_) => identity_unreadable = true,
                }
            }
            // A Flatpak app scope is set by Flatpak/systemd for the app and
            // every process it starts, whatever their names. It is required
            // evidence only for waiving a process we cannot inspect.
            match fs::read_to_string(dir.join("cgroup")) {
                Ok(cgroup) => {
                    if who.scope(&cgroup) {
                        report.emulator_processes.push(pid);
                    }
                    cgroup_text = cgroup;
                }
                Err(_) => cgroup_unreadable = true,
            }
            // Any emulator marker anywhere in the name, command line or cgroup
            // refuses, whether or not the process can otherwise be inspected:
            // a wrapper, AppRun, `emu -L x_libretro.so`, RetroDECK's scope.
            // Refusal only; a missing marker clears nothing.
            if who.marker(&cgroup_text) || tokens_seen.iter().any(|token| who.marker(token)) {
                report.emulator_processes.push(pid);
            }
            if let Ok(executable) = fs::read_link(dir.join("exe"))
                && who.name(&executable.to_string_lossy())
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
                            Err(_) => descriptors_unreadable = true,
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && !dir.exists() => {}
                Err(_) => descriptors_unreadable = true,
            }
            if !dir.exists() {
                continue;
            }
            if identity_unreadable || (descriptors_unreadable && cgroup_unreadable) {
                report.unreadable_processes.push(pid);
            } else if descriptors_unreadable {
                // Without readable descriptors, any emulator marker at all in
                // the command line counts, not only the executable.
                if tokens_seen
                    .iter()
                    .any(|token| who.name(token) || token.contains(who.flatpak_id))
                    || who.marker(&cgroup_text)
                    || tokens_seen.iter().any(|token| who.marker(token))
                {
                    report.emulator_processes.push(pid);
                }
                uninspectable.push(pid);
            }
        }
        report.card_holders.sort_unstable();
        report.card_holders.dedup();
        report.emulator_processes.sort_unstable();
        report.emulator_processes.dedup();
        // Checked after the scan: a writer arriving during it breaks the lease.
        let covered = match lease {
            Some(lease) if lease.intact() => who.lease_waives_uninspectable,
            Some(_) => {
                report.card_open_for_writing = true;
                false
            }
            None => false,
        };
        for pid in uninspectable {
            if report.emulator_processes.contains(&pid) {
                continue;
            }
            if covered {
                report.covered_by_lease.push(pid);
            } else {
                report.unreadable_processes.push(pid);
            }
        }
        report.unreadable_processes.sort_unstable();
        report.covered_by_lease.sort_unstable();
        // "Running" only on emulator identity; holders and lease breaks say
        // that some program has the card, not which.
        report.state = if !report.emulator_processes.is_empty() {
            EmulatorQuiescence::Running
        } else if !report.card_holders.is_empty() || report.card_open_for_writing {
            EmulatorQuiescence::CardInUse
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
    fn observe_under_card_lease(
        &self,
        binding: &DirectorySaveBinding,
        lease: &CardWriteLease,
    ) -> EmulatorQuiescence {
        self.report_under_card_lease(binding, lease).state
    }
}

/// What counts as "the emulator is running" for one binding. Names are only
/// ever used to refuse. Any binding that does not name RetroArch keeps the
/// PS2 identity the card restore was reviewed with.
struct EmulatorIdentity {
    names: &'static [&'static str],
    flatpak_id: &'static str,
    /// An intact card lease may stand in for the descriptors of a process we
    /// cannot inspect, but only when its name, command line and cgroup were
    /// all readable and carry no emulator marker. The lease answers exactly
    /// the question the unreadable descriptors would (does any process have
    /// the save open, or open it for writing), for every uid. Without an
    /// intact lease the process stays `Unknown`. For RetroArch, which opens
    /// its SRAM only briefly, an idle emulator is found by identity (inspected
    /// processes: name and `exe`; uninspectable ones: `markers`), not by the
    /// lease.
    lease_waives_uninspectable: bool,
    /// Substrings (lowercase) that mark a process we cannot inspect as the
    /// emulator or a launcher of it, wherever they appear in its name,
    /// command line or cgroup. Used only for uninspectable processes, where
    /// the alternative is `Unknown`; refusal only, never a clearance.
    markers: &'static [&'static str],
    /// Also match names like `retroarch_vulkan` / `retroarch32` (refusal only).
    loose_suffix: bool,
}

const RETROARCH_PROCESS_NAMES: &[&str] = &["retroarch"];

impl EmulatorIdentity {
    fn for_binding(binding: &DirectorySaveBinding) -> Self {
        if binding.emulator.eq_ignore_ascii_case("retroarch") {
            Self {
                names: RETROARCH_PROCESS_NAMES,
                flatpak_id: "org.libretro.RetroArch",
                lease_waives_uninspectable: true,
                loose_suffix: true,
                markers: &["retroarch", "libretro", "retrodeck"],
            }
        } else {
            Self {
                names: PS2_EMULATOR_PROCESS_NAMES,
                flatpak_id: "net.pcsx2.PCSX2",
                lease_waives_uninspectable: true,
                loose_suffix: false,
                markers: &[],
            }
        }
    }

    fn marker(&self, text: &str) -> bool {
        let text = text.to_ascii_lowercase();
        self.markers.iter().any(|marker| text.contains(marker))
    }

    fn scope(&self, cgroup: &str) -> bool {
        cgroup.contains(&format!("app-flatpak-{}", self.flatpak_id))
    }

    fn name(&self, value: &str) -> bool {
        let name = Path::new(value.trim())
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        self.names.iter().any(|expected| {
            name == *expected
                || name.strip_prefix(expected).is_some_and(|suffix| {
                    suffix.starts_with('-')
                        || suffix == ".appimage"
                        || suffix == ".exe"
                        || (self.loose_suffix
                            && suffix
                                .chars()
                                .next()
                                .is_some_and(|c| c == '_' || c == '.' || c.is_ascii_digit()))
                })
        })
    }
}
