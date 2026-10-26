use super::*;
use crate::{
    emulator_environment::retroarch::ProfileKind,
    launch::{
        RetroArchCommand,
        process_spawn::{self, PreparedProcessCommand, ProcessExitReport, WatchedProcess},
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    io::Write,
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::{
            fs::{FileExt, OpenOptionsExt, PermissionsExt},
            net::UnixStream,
        },
    },
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32},
    },
    time::Duration,
};
const OBSERVER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sram-observer.so"));
struct Lease {
    root: PathBuf,
    file: File,
    _directory: File,
}
impl Lease {
    fn register(root: &Path, id: &str) -> Result<Self, String> {
        use std::os::fd::AsRawFd;
        let directory = File::open(root).map_err(|e| e.to_string())?;
        let pinned = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
        let root = pinned.as_path();
        let result = Self::register_inner(root, id, &directory);
        if result.is_err() {
            use std::os::unix::ffi::OsStrExt;
            if let Ok(path) =
                std::ffi::CString::new(root.join("coordination-uncertain").as_os_str().as_bytes())
            {
                unsafe {
                    libc::mkdir(path.as_ptr(), 0o700);
                }
            }
        }
        result
    }
    fn register_inner(root: &Path, id: &str, directory: &File) -> Result<Self, String> {
        if root.join("coordination-uncertain").exists() {
            return Err("SRAM coordination needs review after an unobserved session".into());
        }
        let coordination = lock(root)?;
        let path = root.join(format!("active-{id}.lease"));
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
            .map_err(|e| e.to_string())?;
        flock(&file, libc::LOCK_EX | libc::LOCK_NB)?;
        file.write_all(b"0").map_err(|e| e.to_string())?;
        let mut count = 0;
        for entry in fs::read_dir(root).map_err(|e| e.to_string())? {
            let p = entry.map_err(|e| e.to_string())?.path();
            if p == path || p.extension().and_then(|s| s.to_str()) != Some("lease") {
                continue;
            }
            count += 1;
            if count > 4096 {
                return Err("Capture lease limit exceeded".into());
            }
            let other = private_rw(&p)?;
            let mut value = [0];
            if other.metadata().map_err(|e| e.to_string())?.len() != 1
                || other.read_at(&mut value, 0).map_err(|e| e.to_string())? != 1
                || !matches!(value[0], b'0' | b'!')
                || !p
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("active-"))
            {
                return Err(
                    "Unknown or malformed session lease; review coordination metadata".into(),
                );
            }
            if flock(&other, libc::LOCK_EX | libc::LOCK_NB).is_ok() {
                let held = Stamp::from_metadata(&other.metadata().map_err(|e| e.to_string())?);
                let named =
                    Stamp::from_metadata(&fs::symlink_metadata(&p).map_err(|e| e.to_string())?);
                if held != named {
                    return Err("Coordination lease identity changed".into());
                }
                fs::remove_file(&p).map_err(|e| e.to_string())?;
            } else {
                other.write_at(b"!", 0).map_err(|e| e.to_string())?;
                file.write_at(b"!", 0).map_err(|e| e.to_string())?;
            }
        }
        drop(coordination);
        let retained = directory.try_clone().map_err(|e| e.to_string())?;
        let own_root = PathBuf::from(format!("/proc/self/fd/{}", retained.as_raw_fd()));
        Ok(Self {
            root: own_root,
            file,
            _directory: retained,
        })
    }

    fn untainted(&self) -> bool {
        let mut b = [0];
        self.file
            .read_at(&mut b, 0)
            .is_ok_and(|n| n == 1 && b[0] == b'0')
    }
}
// Keep the filename after the watched parent exits: an inherited descriptor
// can still hold the flock in a descendant. A later registration reaps only
// unlocked, identity-checked coordination metadata under the global lock.
fn flock(file: &File, how: i32) -> Result<(), String> {
    if unsafe { libc::flock(file.as_raw_fd(), how) } < 0 {
        Err(std::io::Error::last_os_error().to_string())
    } else {
        Ok(())
    }
}
fn private_rw(path: &Path) -> Result<File, String> {
    let f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| e.to_string())?;
    let m = f.metadata().map_err(|e| e.to_string())?;
    let s = Stamp::from_metadata(&m);
    if !m.is_file() || s.links != 1 || s.mode & 0o077 != 0 || s.uid != unsafe { libc::geteuid() } {
        return Err("Unsafe capture coordination file".into());
    }
    Ok(f)
}
struct Lock(File);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = flock(&self.0, libc::LOCK_UN);
    }
}
fn lock(root: &Path) -> Result<Lock, String> {
    let f = private_rw(&root.join("coordination.lock"))?;
    flock(&f, libc::LOCK_EX)?;
    Ok(Lock(f))
}
fn baseline(root: &Path) -> Result<BTreeSet<PathBuf>, String> {
    fn walk(path: &Path, out: &mut BTreeSet<PathBuf>, depth: usize) -> Result<(), String> {
        if depth > 16 || out.len() > 4096 {
            return Err("Save scope exceeds capture bounds".into());
        }
        for e in fs::read_dir(path).map_err(|e| e.to_string())? {
            let p = e.map_err(|e| e.to_string())?.path();
            let m = fs::symlink_metadata(&p).map_err(|e| e.to_string())?;
            if m.file_type().is_symlink() {
                return Err("Linked save scope cannot be attested".into());
            }
            if m.is_dir() {
                walk(&p, out, depth + 1)?;
            } else if m.is_file() {
                out.insert(p);
            } else {
                return Err("Special save scope object refused".into());
            }
        }
        Ok(())
    }
    let mut out = BTreeSet::new();
    walk(root, &mut out, 0)?;
    Ok(out)
}
struct Prepared {
    ctx: Context,
    _objects: Vec<File>,
    store: store::Store,
    session: String,
    lease: Lease,
    before: BTreeSet<PathBuf>,
    eligible: BTreeMap<PathBuf, SramOrigin>,
    _scratch: tempfile::TempDir,
    library: PathBuf,
}
impl Prepared {
    fn new(command: &RetroArchCommand) -> Result<Self, String> {
        if OBSERVER.is_empty()
            || command.selection.profile.profile_kind != ProfileKind::Native
            || command.selection.profile.scope
                != crate::emulator_environment::retroarch::ProfileScope::User
            || command.selection.core_stem != "gambatte"
            || !matches!(
                command.selection.platform_id.as_str(),
                "Game Boy" | "Game Boy Color"
            )
        {
            return Err("Automatic SRAM observation is not supported for this launch".into());
        }
        if std::env::vars_os().any(|(k, v)| k.to_string_lossy().starts_with("LD_") && !v.is_empty())
        {
            return Err(
                "Existing dynamic-loader overrides prevent trusted SRAM observation".into(),
            );
        }
        let scope = discover_scope(command.selection.profile)?;
        if command
            .arguments
            .windows(2)
            .any(|w| w[0] == "--config" && Path::new(&w[1]) != scope.configuration.path)
        {
            return Err("Launch config differs from discovered scope".into());
        }
        if command
            .arguments
            .iter()
            .any(|a| a == "--appendconfig" || a == "--subsystem")
        {
            return Err("Extended launch configuration cannot be attested".into());
        }
        let report = crate::game_identity::inspect_catalogued_game_identity(
            &command.selection.content_path,
            Some(&command.selection.platform_id),
        );
        let game = report
            .evidence
            .iter()
            .find(|e| {
                e.kind == crate::game_identity::IdentityKind::LooseRomSha256
                    && e.status == crate::game_identity::IdentityStatus::Verified
            })
            .ok_or("Fresh verified cartridge bytes unavailable")?
            .clone();
        let (cartridge, rom) = object(&command.selection.content_path)?;
        if Some(&cartridge.sha256) != game.value.as_ref() {
            return Err("Cartridge changed during observation preparation".into());
        }
        let (executable, exe) = object(&command.executable)?;
        let (core, library_file) = object(&command.selection.core_library)?;
        if scope.selected_executable != executable.path || scope.selected_core != core.path {
            return Err(
                "Selected native installation/core changed during capture preparation".into(),
            );
        }
        let ctx = Context {
            cartridge,
            cartridge_identity: game,
            executable,
            core,
            scope,
        };
        let store = store::Store::open(&catalogue_root()?, true)?;
        let session = store::session_id()?;
        let lease = Lease::register(&store.root, &session)?;
        let before = baseline(&ctx.scope.saves)?;
        let mut eligible = BTreeMap::new();
        let mut claims: BTreeMap<PathBuf, Context> = BTreeMap::new();
        for receipt in store.receipts()? {
            for origin in receipt.origins {
                if let Some(previous) =
                    claims.insert(origin.artifact.path.clone(), origin.context.clone())
                    && previous != origin.context
                {
                    return Err("Conflicting historical artifact ownership".into());
                }
                if origin.context == ctx && current(&origin.artifact).is_ok() {
                    eligible.insert(origin.artifact.path.clone(), origin);
                }
            }
        }
        let scratch = tempfile::Builder::new()
            .prefix("emuwiz-sram-observer-")
            .tempdir()
            .map_err(|e| e.to_string())?;
        let library = scratch.path().join("observer.so");
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&library)
            .map_err(|e| e.to_string())?;
        f.write_all(OBSERVER)
            .and_then(|_| f.sync_all())
            .map_err(|e| e.to_string())?;
        fs::set_permissions(&library, fs::Permissions::from_mode(0o500))
            .map_err(|e| e.to_string())?;
        store.publish(
            &format!("{session}.pending"),
            &Receipt {
                version: 1,
                session: session.clone(),
                context: ctx.clone(),
                origins: Vec::new(),
                reason: Some(
                    "Capture pending; no association until a complete witnessed exit".into(),
                ),
            },
        )?;
        Ok(Self {
            ctx: ctx.clone(),
            _objects: vec![
                rom,
                exe,
                library_file,
                open_directory(&ctx.scope.saves, false)?,
                open_object(&ctx.scope.configuration.path)?,
            ],
            store,
            session,
            lease,
            before,
            eligible,
            _scratch: scratch,
            library,
        })
    }
    fn finish(self, report: &ProcessExitReport, transcript: Result<protocol::Transcript, String>) {
        let mut reason = None;
        let mut origins = Vec::new();
        let coordination = lock(&self.lease.root);
        let result = (|| -> Result<(), String> {
            if self.store.root.join("coordination-uncertain").exists() {
                return Err("SRAM session coordination is uncertain; no association".into());
            }
            self.store.revalidate_key()?;
            if coordination.is_err() {
                return Err("Capture coordination became unavailable".into());
            }
            if !report.status.as_ref().is_ok_and(|s| s.success()) {
                return Err(
                    "Interrupted or unsuccessful emulator session; no new provenance".into(),
                );
            }
            if !self.lease.untainted() {
                return Err("Concurrent Blue emulator sessions; no new provenance".into());
            }
            current(&self.ctx.cartridge)?;
            current(&self.ctx.core)?;
            current(&self.ctx.executable)?;
            current(&self.ctx.scope.configuration)?;
            if discover_scope(crate::emulator_environment::retroarch::ProfileRef {
                profile_kind: ProfileKind::Native,
                scope: crate::emulator_environment::retroarch::ProfileScope::User,
            })? != self.ctx.scope
            {
                return Err("Selected save configuration scope changed".into());
            }
            let t = transcript?;
            if !t.complete() {
                return Err(t
                    .reason
                    .unwrap_or_else(|| "Incomplete native observer lifecycle".into()));
            }
            for artifact in t.artifacts.into_values() {
                if self.before.contains(&artifact.path)
                    && !self.eligible.contains_key(&artifact.path)
                {
                    continue;
                }
                current(&artifact)?;
                origins.push(SramOrigin{state:OriginState::Associated,session:self.session.clone(),context:self.ctx.clone(),artifact,explanation:"Blue observed this verified cartridge's SRAM being written to this file. These exact bytes are linked to that session. Restore and save-state compatibility require separate checks.".into()});
            }
            if origins.is_empty() {
                return Err(
                    "No eligible full SRAM write was observed; historical saves remain unassigned"
                        .into(),
                );
            }
            Ok(())
        })();
        if let Err(e) = result {
            reason = Some(e);
            origins.clear();
        }
        let receipt = Receipt {
            version: 1,
            session: self.session.clone(),
            context: self.ctx.clone(),
            origins,
            reason,
        };
        if self
            .store
            .publish(&format!("{}.json", self.session), &receipt)
            .is_ok()
        {
            let _ = fs::remove_file(self.store.root.join(format!("{}.pending", self.session)));
        }
        REVISION.fetch_add(1, Ordering::Release);
    }
}
/// Preparation happens before the launch executor's final source/asset guards.
/// Every native launch must retain coordination even when capture is unsupported.
pub(crate) struct LaunchObservation {
    observed: Option<Prepared>,
    unobserved: Option<(store::Store, Lease)>,
}
#[cfg(target_os = "linux")]
pub(crate) fn prepare(command: &RetroArchCommand) -> std::io::Result<LaunchObservation> {
    match Prepared::new(command) {
        Ok(observed) => Ok(LaunchObservation {
            observed: Some(observed),
            unobserved: None,
        }),
        Err(reason) => {
            super::diagnostic(reason);
            let unobserved = if command.selection.profile.profile_kind == ProfileKind::Native {
                let result = (|| -> Result<_, String> {
                    let store = store::Store::open(&catalogue_root()?, true)?;
                    let lease = Lease::register(&store.root, &store::session_id()?)?;
                    Ok((store, lease))
                })();
                Some(result.map_err(|e|std::io::Error::other(format!("Native save-session coordination unavailable; launch refused to avoid an untracked session: {e}")))?)
            } else {
                None
            };
            Ok(LaunchObservation {
                observed: None,
                unobserved,
            })
        }
    }
}
#[cfg(not(target_os = "linux"))]
pub(crate) fn prepare(_command: &RetroArchCommand) -> std::io::Result<LaunchObservation> {
    Ok(LaunchObservation {
        observed: None,
        unobserved: None,
    })
}
impl LaunchObservation {
    pub(crate) fn spawn(
        self,
        prepared: &PreparedProcessCommand,
    ) -> std::io::Result<WatchedProcess> {
        #[cfg(target_os = "linux")]
        {
            match self.observed {
                Some(observation) => spawn_observed(observation, prepared),
                None => spawn_unobserved(self.unobserved, prepared),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            process_spawn::spawn_watched_process(prepared)
        }
    }
}
#[cfg(target_os = "linux")]
fn spawn_observed(
    observation: Prepared,
    prepared: &PreparedProcessCommand,
) -> std::io::Result<WatchedProcess> {
    let (parent, child) = UnixStream::pair()?;
    let child: File = OwnedFd::from(child).into();
    let mut cookie = 0u64;
    let mut len = std::mem::size_of::<u64>() as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            child.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_COOKIE,
            (&mut cookie as *mut u64).cast(),
            &mut len,
        )
    } < 0
    {
        super::diagnostic(
            "Native socket identity unavailable; automatic SRAM capture disabled".into(),
        );
        let Prepared { store, lease, .. } = observation;
        return spawn_unobserved(Some((store, lease)), prepared);
    }
    let environment: Vec<(OsString, OsString)> = vec![
        (
            "LD_PRELOAD".into(),
            observation.library.clone().into_os_string(),
        ),
        (
            "EMUWIZ_SRAM_OBSERVER_FD".into(),
            child.as_raw_fd().to_string().into(),
        ),
        ("EMUWIZ_SRAM_COOKIE".into(), cookie.to_string().into()),
        (
            "EMUWIZ_SRAM_ROOT".into(),
            observation.ctx.scope.saves.clone().into_os_string(),
        ),
        (
            "EMUWIZ_SRAM_CONFIG".into(),
            observation
                .ctx
                .scope
                .configuration
                .path
                .clone()
                .into_os_string(),
        ),
        (
            "EMUWIZ_SRAM_CORE".into(),
            observation.ctx.core.path.clone().into_os_string(),
        ),
    ];
    let pid = Arc::new(AtomicU32::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let (send, receive) = std::sync::mpsc::channel();
    let reader_pid = pid.clone();
    let reader_stop = stop.clone();
    let ctx = observation.ctx.clone();
    let eligible = observation.eligible.clone();
    let lease_copy = observation.lease.file.try_clone()?;
    std::thread::Builder::new()
        .name("sram-origin-observer".into())
        .spawn(move || {
            let result = protocol::read(parent, ctx, eligible, reader_pid, reader_stop);
            let _ = send.send(result);
        })?;
    let failure_stop = stop.clone();
    let result = process_spawn::spawn_watched_process_observed(
        prepared,
        &environment,
        &[&child, &lease_copy],
        move |n| pid.store(n, Ordering::Release),
        move |report| {
            let result = receive
                .recv_timeout(Duration::from_secs(2))
                .map_err(|_| "Observer stream did not finish with the emulator".into());
            stop.store(true, Ordering::Release);
            observation.finish(report, result);
        },
    );
    if result.is_err() {
        failure_stop.store(true, Ordering::Release);
    }
    result
}
#[cfg(target_os = "linux")]
fn spawn_unobserved(
    ownership: Option<(store::Store, Lease)>,
    prepared: &PreparedProcessCommand,
) -> std::io::Result<WatchedProcess> {
    if let Some((store, lease)) = ownership {
        let file = lease.file.try_clone()?;
        return process_spawn::spawn_watched_process_observed(
            prepared,
            &[],
            &[&file],
            |_| {},
            move |_| {
                drop(lease);
                drop(store);
                REVISION.fetch_add(1, Ordering::Release);
            },
        );
    }
    process_spawn::spawn_watched_process(prepared)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Other libtest threads can fork between a local close and the next lease
    // registration. CLOEXEC closes those accidental copies only at exec. Run
    // these close/reap assertions after exec in a single-test process, without
    // changing the production rule that any live inherited lease is tainted.
    fn in_lease_test_process(name: &str) -> bool {
        const CHILD: &str = "EMUWIZ_SRAM_LEASE_TEST_CHILD";
        let name = format!("sram_provenance::capture::tests::{name}");
        if std::env::var(CHILD).ok().as_deref() == Some(name.as_str()) {
            return true;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &name,
                "--nocapture",
                "--test-threads=1",
                "--color",
                "never",
            ])
            .env(CHILD, &name)
            .output()
            .unwrap();
        assert!(
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed;"),
            "{name}:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        false
    }

    #[test]
    fn inherited_lease_stays_visible_after_parent_completion() {
        if !in_lease_test_process("inherited_lease_stays_visible_after_parent_completion") {
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let store = store::Store::open(&t.path().join("origins"), true).unwrap();
        let first = Lease::register(&store.root, "first").unwrap();
        let inherited = first.file.try_clone().unwrap();
        drop(first);
        let second = Lease::register(&store.root, "second").unwrap();
        assert!(
            !second.untainted(),
            "live inherited ownership must remain discoverable"
        );
        drop(second);
        drop(inherited);
        let third = Lease::register(&store.root, "third").unwrap();
        assert!(third.untainted());
    }
    #[test]
    fn concurrent_sessions_are_tainted_and_leases_remain_pinned() {
        if !in_lease_test_process("concurrent_sessions_are_tainted_and_leases_remain_pinned") {
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let store = store::Store::open(&t.path().join("origins"), true).unwrap();
        let first = Lease::register(&store.root, "first").unwrap();
        assert!(first.untainted());
        let second = Lease::register(&store.root, "second").unwrap();
        assert!(!first.untainted());
        assert!(!second.untainted());
        drop(first);
        drop(second);
        let third = Lease::register(&store.root, "third").unwrap();
        assert!(third.untainted());
    }

    #[test]
    fn forked_pre_exec_holder_keeps_lease_tainted_until_exec() {
        use std::os::unix::process::CommandExt;
        if !in_lease_test_process("forked_pre_exec_holder_keeps_lease_tainted_until_exec") {
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let store = store::Store::open(&t.path().join("origins"), true).unwrap();
        let first = Lease::register(&store.root, "first").unwrap();
        let (parent, child) = UnixStream::pair().unwrap();
        let fork = std::thread::spawn(move || {
            let fd = child.as_raw_fd();
            let mut command = std::process::Command::new("/bin/true");
            // SAFETY: the fork child uses only async-signal-safe libc calls
            // until exec. The socket handshake holds a deterministic pre-exec
            // window; the alarm bounds failures without timing the assertion.
            unsafe {
                command.pre_exec(move || {
                    libc::alarm(10);
                    let ready = [1u8];
                    if libc::write(fd, ready.as_ptr().cast(), 1) != 1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    let mut release = [0u8];
                    if libc::read(fd, release.as_mut_ptr().cast(), 1) != 1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    libc::alarm(0);
                    Ok(())
                });
            }
            let result = command.spawn().and_then(|mut process| process.wait());
            drop(child);
            result
        });
        // Release the child even if an assertion unwinds, so a failed safety
        // regression never leaves a deliberately blocked fork behind.
        struct Release(UnixStream);
        impl Drop for Release {
            fn drop(&mut self) {
                let _ = self.0.write_all(&[1]);
            }
        }
        let mut release = Release(parent);
        let mut ready_fd = libc::pollfd {
            fd: release.0.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // The only reader owns this socket; once ready, read_exact cannot
        // wait for another consumer. Bound the handshake, not lease cleanup.
        assert_eq!(unsafe { libc::poll(&mut ready_fd, 1, 10_000) }, 1);
        let mut ready = [0u8];
        release.0.read_exact(&mut ready).unwrap();
        assert_eq!(ready, [1]);
        drop(first);
        let second = Lease::register(&store.root, "second").unwrap();
        assert!(
            !second.untainted(),
            "a fork-inherited CLOEXEC lease is still live before exec"
        );
        drop(second);
        drop(release);
        assert!(fork.join().unwrap().unwrap().success());
        let third = Lease::register(&store.root, "third").unwrap();
        assert!(third.untainted(), "exec/exit released the inherited lease");
    }
}
