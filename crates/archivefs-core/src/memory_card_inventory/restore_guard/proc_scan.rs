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
                        // Structured launch forms, read from where they sit in
                        // the command line, never from a word somewhere in it.
                        if leaf == "cmdline" && who.launch_arguments(&tokens) {
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
            // A marker in the cgroup refuses: the cgroup is set by the system
            // (RetroDECK's app scope), not typed by anyone.
            if who.marker(&cgroup_text) {
                report.emulator_processes.push(pid);
            }
            // A process is the emulator by what it *is*: its name, the file
            // it runs, how it was launched. A word in its command line is
            // not identity: `cargo test retroarch_journey_tests`, an editor
            // open on `retroarch.cfg` or a script given `--only retroarch`
            // are not RetroArch (found 2026-11-20: three such processes of
            // other sessions made every RetroArch save read "in use").
            //
            // The word match is kept only for a process whose executable
            // cannot be read (another user's, a container's): it cannot be
            // classified, so anything naming the emulator still refuses.
            // A process that has simply gone is not such a process.
            let executable = fs::read_link(dir.join("exe"));
            let executable_hidden = matches!(
                &executable,
                Err(error) if error.kind() != std::io::ErrorKind::NotFound
            ) && tokens_seen
                .iter()
                .skip(1)
                .any(|token| !token.trim().is_empty());
            if executable_hidden && tokens_seen.iter().any(|token| who.marker(token)) {
                report.emulator_processes.push(pid);
            }
            // After an update or a replaced AppImage the kernel shows the
            // running file as "<path> (deleted)": still the same program.
            if let Ok(executable) = executable
                && who.name(executable.to_string_lossy().trim_end_matches(" (deleted)"))
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
/// ever used to refuse. A binding that names RetroArch, DuckStation, Dolphin,
/// RPCS3 or PPSSPP gets that emulator's identity; every other binding keeps
/// the PS2 identity the card restore was reviewed with.
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
    /// A program started with `-L <something>_libretro.so` is a libretro
    /// front end running a core, whatever the program is called.
    libretro_core_argument: bool,
}

/// Programs that run a script named in their first argument.
const SCRIPT_RUNNERS: &[&str] = &[
    "sh", "bash", "dash", "zsh", "ksh", "env", "python", "python3", "perl",
];

const RETROARCH_PROCESS_NAMES: &[&str] = &["retroarch"];

impl EmulatorIdentity {
    fn for_binding(binding: &DirectorySaveBinding) -> Self {
        if binding.emulator.eq_ignore_ascii_case("retroarch") {
            Self {
                names: RETROARCH_PROCESS_NAMES,
                flatpak_id: "org.libretro.RetroArch",
                lease_waives_uninspectable: true,
                loose_suffix: true,
                libretro_core_argument: true,
                markers: &["retroarch", "libretro", "retrodeck"],
            }
        } else if let Some(identity) = Self::other_emulator(&binding.emulator) {
            identity
        } else {
            Self {
                names: PS2_EMULATOR_PROCESS_NAMES,
                flatpak_id: "net.pcsx2.PCSX2",
                lease_waives_uninspectable: true,
                loose_suffix: false,
                libretro_core_argument: false,
                markers: &[],
            }
        }
    }

    /// Emulators added for save-continuity snapshots. Each is matched by its
    /// own executable names (native, AppImage and the binary an AppImage
    /// mounts), its Flatpak app id and scope, and, for processes that cannot
    /// be inspected, a marker anywhere in the name, command line or cgroup.
    ///
    /// The names are deliberately the emulator's own: plain `dolphin` is
    /// KDE's file manager and is not listed. A name that is missing here
    /// cannot clear anything: an unlisted emulator build that keeps the save
    /// open is still caught by the kernel lease, and without a lease an
    /// uninspectable process keeps the answer `Unknown`.
    fn other_emulator(emulator: &str) -> Option<Self> {
        let (names, flatpak_id, markers): (&'static [&'static str], _, &'static [&'static str]) =
            match emulator.to_ascii_lowercase().as_str() {
                "duckstation" => (
                    &["duckstation"],
                    "org.duckstation.DuckStation",
                    &["duckstation"],
                ),
                "dolphin" => (
                    &[
                        "dolphin-emu",
                        "dolphin-emu-nogui",
                        "dolphin-emu-qt",
                        "primehack",
                    ],
                    "org.DolphinEmu.dolphin-emu",
                    &["dolphin-emu", "dolphinemu", "dolphin_emu"],
                ),
                "rpcs3" => (&["rpcs3"], "net.rpcs3.RPCS3", &["rpcs3"]),
                "ppsspp" => (
                    &["ppsspp", "ppssppsdl", "ppssppqt", "ppssppheadless"],
                    "org.ppsspp.PPSSPP",
                    &["ppsspp"],
                ),
                _ => return None,
            };
        Some(Self {
            names,
            flatpak_id,
            lease_waives_uninspectable: true,
            loose_suffix: true,
            libretro_core_argument: false,
            markers,
        })
    }

    /// The emulator, recognised from the *shape* of a command line:
    ///
    /// - a script runner whose script (its first argument that is not an
    ///   option) is named like the emulator: `bash ~/bin/retroarch`,
    ///   `python3 /opt/pcsx2-qt-wrapper`;
    /// - for RetroArch, a core handed over with `-L x_libretro.so` or
    ///   `--libretro=x_libretro.so`.
    ///
    /// A word elsewhere (a test filter, a file being edited, a folder name,
    /// text given to `-c`) is not looked at.
    fn launch_arguments(&self, tokens: &[String]) -> bool {
        let program = tokens
            .first()
            .and_then(|token| Path::new(token.trim()).file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if SCRIPT_RUNNERS.contains(&program) {
            // The script is the first argument that is not an option, a
            // `NAME=value` setting, or another runner (`env bash script`).
            let script = tokens.iter().skip(1).find(|token| {
                let runner = Path::new(token.as_str())
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| SCRIPT_RUNNERS.contains(&name));
                !token.starts_with('-') && !token.contains('=') && !runner
            });
            // `sh -c "<text>"`: what follows -c is text to run, not a script.
            let inline = tokens.iter().skip(1).any(|token| token == "-c");
            if !inline && script.is_some_and(|script| self.name(script)) {
                return true;
            }
        }
        // A program that runs another program named in its arguments: the
        // dynamic loader (`/lib64/ld-linux-x86-64.so.2 /path/retroarch`, how
        // some bundled builds start), or a tool that runs a binary under it.
        // Its own name and executable say nothing, so each argument is
        // tested as a program name.
        let runs_a_program = program.starts_with("ld-")
            || program.starts_with("ld.so")
            || program.starts_with("qemu-")
            || matches!(
                program,
                "valgrind" | "box64" | "box86" | "strace" | "gdb" | "gamemoderun" | "mangohud"
            );
        if runs_a_program {
            // The program it runs is its first argument that is not one of
            // its own options (nor the value of an option that takes one).
            // Only that argument is tested: anything after it belongs to the
            // program being run (`ld.so python3 notes.py retroarch` runs
            // python, not RetroArch).
            let mut arguments = tokens.iter().skip(1);
            while let Some(token) = arguments.next() {
                if matches!(
                    token.as_str(),
                    "--argv0" | "--library-path" | "--preload" | "--inhibit-rpath" | "--audit"
                ) {
                    arguments.next();
                    continue;
                }
                if token.starts_with('-') {
                    continue;
                }
                return self.name(token);
            }
        }
        if self.libretro_core_argument {
            let is_core = |token: &str| token.to_ascii_lowercase().ends_with("_libretro.so");
            // `-L core`, `--libretro core`, and bundled short options (`-fL core`).
            let paired = tokens.windows(2).any(|pair| {
                let option = pair[0].as_str();
                let short_bundle =
                    option.starts_with('-') && !option.starts_with("--") && option.ends_with('L');
                (short_bundle || option == "--libretro") && is_core(&pair[1])
            });
            // `-L/path/core`, `--libretro=core`, and getopt's shortened
            // `--libr=core`.
            let joined = tokens.iter().any(|token| {
                let attached_short = token.starts_with("-L") && !token.starts_with("--");
                let attached_long = token.starts_with("--libr");
                (attached_short || attached_long) && is_core(token)
            });
            return paired || joined;
        }
        false
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
