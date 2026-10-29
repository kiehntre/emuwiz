//! Selected loose GB/GBC/GBA launch through Core's native mGBA preflight.
use super::*;
use archivefs_core::launch::mgba_execution::{capture_mgba_config, capture_mgba_executable};
use archivefs_core::launch::process_spawn::{CapturedFileIdentity, WatchedProcess};
use archivefs_core::launch::{MgbaLaunchRequest, preflight_mgba_launch, spawn_mgba};
use archivefs_core::patch_manager::{
    MgbaProfileDiscoveryRoots, discover_mgba_profiles, resolve_mgba_native_launch_binding,
};
use archivefs_core::platform_evidence_fusion::FusionOutcome;
#[cfg(target_os = "linux")]
use sha2::{Digest, Sha256};
#[cfg(target_os = "linux")]
use std::io::Read;
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};
#[cfg(target_os = "linux")]
use std::sync::{Mutex, OnceLock, mpsc};
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
const BWRAP: &str = "/usr/bin/bwrap";
#[cfg(target_os = "linux")]
const PROBE_LIMIT: usize = 64 * 1024;
#[cfg(target_os = "linux")]
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(target_os = "linux")]
const CHECKING: &str = "Checking Bubblewrap's required launch options…";

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BubblewrapReceipt {
    identity: CapturedFileIdentity,
    sha256: [u8; 32],
}

#[cfg(target_os = "linux")]
enum ProbeState {
    Pending(
        CapturedFileIdentity,
        bool,
        mpsc::Receiver<Result<BubblewrapReceipt, String>>,
    ),
    Complete(
        CapturedFileIdentity,
        bool,
        Instant,
        Result<BubblewrapReceipt, String>,
    ),
}

#[cfg(target_os = "linux")]
#[derive(Default)]
struct ProbeCache(Option<ProbeState>);

#[cfg(target_os = "linux")]
impl ProbeCache {
    fn offer(&mut self, path: &Path, bind_fd: bool) -> Result<BubblewrapReceipt, String> {
        let identity = match bubblewrap_identity(path) {
            Ok(identity) => identity,
            Err(reason) => {
                self.0 = None;
                return Err(reason);
            }
        };
        let same = match &self.0 {
            Some(ProbeState::Pending(previous, required, _)) => {
                *previous == identity && *required == bind_fd
            }
            Some(ProbeState::Complete(previous, required, at, result)) => {
                *previous == identity
                    && *required == bind_fd
                    && (result.is_ok() || at.elapsed() < Duration::from_secs(3))
            }
            None => false,
        };
        if !same {
            let (sender, receiver) = mpsc::channel();
            let path = path.to_path_buf();
            std::thread::spawn(move || {
                let _ = sender.send(probe_bubblewrap(&path, bind_fd));
            });
            self.0 = Some(ProbeState::Pending(identity, bind_fd, receiver));
        }
        if let Some(ProbeState::Pending(_, _, receiver)) = &self.0 {
            match receiver.try_recv() {
                Ok(result) => {
                    self.0 = Some(ProbeState::Complete(
                        identity,
                        bind_fd,
                        Instant::now(),
                        result,
                    ))
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.0 = Some(ProbeState::Complete(
                        identity,
                        bind_fd,
                        Instant::now(),
                        Err("Bubblewrap capability check failed. Recheck readiness.".into()),
                    ));
                }
                Err(mpsc::TryRecvError::Empty) => return Err(CHECKING.into()),
            }
        }
        match &self.0 {
            Some(ProbeState::Complete(_, _, _, result)) => result.clone(),
            _ => Err(CHECKING.into()),
        }
    }
}

#[cfg(target_os = "linux")]
fn bubblewrap_cache() -> &'static Mutex<ProbeCache> {
    static CACHE: OnceLock<Mutex<ProbeCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(ProbeCache::default()))
}

#[cfg(target_os = "linux")]
fn profile_uses_bind_fd(candidate: &LaunchCandidate) -> Result<bool, String> {
    let LaunchTarget::Standalone { profile_id, .. } = &candidate.target else {
        return Err("The selected candidate is not native mGBA.".into());
    };
    let root = profile_id
        .strip_prefix("mgba:")
        .ok_or("The selected mGBA profile is invalid. Recheck emulator setup.")?;
    // Mirrors Core's mGBA config discovery; Core re-discovers and preflights it at launch.
    Ok(["config.ini", "config.ini.bak"].into_iter().any(|name| {
        std::fs::symlink_metadata(Path::new(root).join(name))
            .is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
    }))
}

#[cfg(target_os = "linux")]
fn bubblewrap_identity(path: &Path) -> Result<CapturedFileIdentity, String> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::symlink_metadata(path).map_err(|_| {
        "Bubblewrap is required for a file-safe native mGBA launch. Install it and recheck readiness.".to_string()
    })?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o111 == 0
    {
        return Err(
            "Bubblewrap must be an executable, regular non-symlink file. Recheck readiness.".into(),
        );
    }
    if metadata.len() > 16 * 1024 * 1024 {
        return Err("Bubblewrap is too large for the bounded capability check.".into());
    }
    Ok(CapturedFileIdentity::capture(&metadata))
}

#[cfg(target_os = "linux")]
fn bubblewrap_digest(path: &Path) -> Result<BubblewrapReceipt, String> {
    let identity = bubblewrap_identity(path)?;
    let mut file = archivefs_core::launch::process_spawn::open_regular_file_nonblocking(path)
        .map_err(|e| format!("Bubblewrap could not be opened safely: {e}"))?;
    if CapturedFileIdentity::capture(&file.metadata().map_err(|e| e.to_string())?) != identity {
        return Err("Bubblewrap changed during its capability check. Recheck readiness.".into());
    }
    // A digest catches in-place replacement even when size and mtime are restored.
    let mut hasher = Sha256::new();
    let mut bytes = [0_u8; 8192];
    let mut total = 0_u64;
    loop {
        let count = file.read(&mut bytes).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > 16 * 1024 * 1024 {
            return Err("Bubblewrap grew beyond the bounded capability-check limit.".into());
        }
        hasher.update(&bytes[..count]);
    }
    if bubblewrap_identity(path)? != identity
        || CapturedFileIdentity::capture(&file.metadata().map_err(|e| e.to_string())?) != identity
    {
        return Err("Bubblewrap changed during its capability check. Recheck readiness.".into());
    }
    Ok(BubblewrapReceipt {
        identity,
        sha256: hasher.finalize().into(),
    })
}

#[cfg(target_os = "linux")]
fn bounded_read(mut stream: impl Read) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    let mut bytes = [0_u8; 4096];
    let mut exceeded = false;
    loop {
        let count = stream.read(&mut bytes).map_err(|e| e.to_string())?;
        if count == 0 {
            return if exceeded {
                Err("Bubblewrap help output exceeded the capability-check limit.".into())
            } else {
                Ok(output)
            };
        }
        if output.len() + count > PROBE_LIMIT {
            exceeded = true;
        } else if !exceeded {
            output.extend_from_slice(&bytes[..count]);
        }
    }
}

#[cfg(target_os = "linux")]
fn probe_bubblewrap(path: &Path, bind_fd: bool) -> Result<BubblewrapReceipt, String> {
    let before = bubblewrap_digest(path)?;
    let mut child = Command::new(path)
        .arg("--help")
        .env_clear()
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Bubblewrap capability check could not start: {e}"))?;
    let (sender, receiver) = mpsc::channel();
    let streams: [Box<dyn Read + Send>; 2] = [
        Box::new(child.stdout.take().unwrap()),
        Box::new(child.stderr.take().unwrap()),
    ];
    for stream in streams {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let _ = sender.send(bounded_read(stream));
        });
    }
    drop(sender);
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Bubblewrap capability check timed out. Recheck readiness.".into());
            }
            Err(e) => return Err(format!("Bubblewrap capability check failed: {e}")),
        }
    };
    let remaining = deadline.saturating_duration_since(Instant::now());
    let out = receiver
        .recv_timeout(remaining)
        .map_err(|_| "Bubblewrap help capture timed out.")??;
    let remaining = deadline.saturating_duration_since(Instant::now());
    let err = receiver
        .recv_timeout(remaining)
        .map_err(|_| "Bubblewrap help capture timed out.")??;
    if !status.success() {
        return Err(
            "Bubblewrap could not report its launch options. Recheck its installation.".into(),
        );
    }
    let output = [out, err].concat();
    let help = String::from_utf8_lossy(&output);
    for option in [
        "--ro-bind-data",
        "--bind",
        "--dev-bind",
        "--proc",
        "--perms",
    ]
    .into_iter()
    .chain(bind_fd.then_some("--bind-fd"))
    {
        if !help.split_whitespace().any(|word| word == option) {
            return Err(format!(
                "Bubblewrap lacks required {option} support. Install a build with this option and recheck readiness."
            ));
        }
    }
    if bubblewrap_digest(path)? != before {
        return Err("Bubblewrap changed during its capability check. Recheck readiness.".into());
    }
    Ok(before)
}

#[cfg(target_os = "linux")]
fn recheck_bubblewrap(path: &Path, ready: BubblewrapReceipt, bind_fd: bool) -> Result<(), String> {
    if probe_bubblewrap(path, bind_fd)? != ready {
        return Err(
            "Bubblewrap changed since Play became ready. Recheck readiness before launching."
                .into(),
        );
    }
    Ok(())
}

#[derive(Clone)]
pub(crate) struct Request {
    plan: LaunchPlan,
    candidate: LaunchCandidate,
    #[cfg(target_os = "linux")]
    bubblewrap: BubblewrapReceipt,
}

struct Reviewed {
    core: MgbaLaunchRequest,
    roots: MgbaProfileDiscoveryRoots,
}

impl Request {
    pub(crate) fn offer(plan: &LaunchPlan, candidate: &LaunchCandidate) -> Result<Self, String> {
        #[cfg(not(target_os = "linux"))]
        return Err("Bound native mGBA launch currently requires Linux and Bubblewrap.".into());
        if !matches!(
            &candidate.target,
            LaunchTarget::Standalone {
                adapter_id: "mgba",
                ..
            }
        ) {
            return Err("The selected candidate is not native mGBA.".into());
        }
        if candidate.readiness != LaunchReadiness::Ready
            || !candidate.blockers.is_empty()
            || !candidate.warnings.is_empty()
            || candidate.content.requires_mount
            || candidate.content.container != Some(LaunchContainerKind::PlainFile)
        {
            return Err("Only a ready, direct loose cartridge can launch in native mGBA. Resolve the listed readiness blockers first.".into());
        }
        let path = candidate
            .content
            .resolved_path
            .as_ref()
            .ok_or("No direct cartridge path is available.")?;
        let platform = plan
            .platform_id
            .as_deref()
            .ok_or("Game platform is unresolved.")?;
        let _ = plan
            .game_key
            .as_ref()
            .ok_or("Game identity is unresolved.")?;
        let extension = match platform {
            "Game Boy" => "gb",
            "Game Boy Color" => "gbc",
            "Game Boy Advance" => "gba",
            _ => return Err("Native mGBA supports only GB, GBC and GBA games.".into()),
        };
        if !path.is_absolute()
            || !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(extension))
        {
            return Err("Native mGBA needs a direct .gb, .gbc or .gba file matching the selected platform; archives are unsupported.".into());
        }
        #[cfg(target_os = "linux")]
        let bubblewrap = bubblewrap_cache()
            .lock()
            .unwrap()
            .offer(Path::new(BWRAP), profile_uses_bind_fd(candidate)?)?;
        Ok(Self {
            plan: plan.clone(),
            candidate: candidate.clone(),
            #[cfg(target_os = "linux")]
            bubblewrap,
        })
    }
    pub(crate) fn key(&self) -> (PathBuf, String) {
        (
            self.candidate
                .content
                .resolved_path
                .clone()
                .unwrap_or_default(),
            "mgba".into(),
        )
    }
    pub(crate) fn execute(self) -> Result<WatchedProcess, String> {
        let roots = MgbaProfileDiscoveryRoots::from_environment().map_err(|e| e.to_string())?;
        self.execute_with_roots(roots)
    }
    fn execute_with_roots(
        self,
        roots: MgbaProfileDiscoveryRoots,
    ) -> Result<WatchedProcess, String> {
        let reviewed = review(&self.plan, &self.candidate, roots)?;
        let identity = fresh_identity(
            &reviewed.core.selected_content_path,
            &reviewed.core.expected_platform_id,
        )?;
        let command =
            preflight_mgba_launch(&reviewed.core, &reviewed.roots, &identity).map_err(|e| {
                format!(
                    "mGBA was not started: {}. Recheck this game and emulator profile.",
                    e.detail
                )
            })?;
        #[cfg(target_os = "linux")]
        recheck_bubblewrap(
            Path::new(BWRAP),
            self.bubblewrap,
            reviewed.core.expected_config_path.is_some(),
        )?;
        spawn_mgba(&command, &reviewed.core).map_err(|e| format!("mGBA could not start: {e}"))
    }
}

pub(super) fn offer_for(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
) -> Option<Result<Request, String>> {
    matches!(
        &candidate.target,
        LaunchTarget::Standalone {
            adapter_id: "mgba",
            ..
        }
    )
    .then(|| Request::offer(plan, candidate))
}

pub(super) fn show_offer(
    ui: &mut egui::Ui,
    state: &mut StandaloneLaunchState,
    offer: Result<Request, String>,
) {
    ui.add_space(6.0);
    match offer {
        Ok(request) => {
            super::show_standalone_launch_action(ui, state, StandaloneLaunchRequest::Mgba(request))
        }
        Err(reason) => {
            #[cfg(target_os = "linux")]
            if reason == CHECKING {
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            } else if reason.starts_with("Bubblewrap") {
                ui.ctx().request_repaint_after(Duration::from_secs(3));
            }
            ui.colored_label(ui.visuals().warn_fg_color, reason);
        }
    }
}

pub(super) fn show_running(ui: &mut egui::Ui, process: &WatchedProcess) {
    ui.small(format!("mGBA running · PID {}", process.pid));
    ui.small("Starting the process does not prove the game ran.");
}

pub(super) fn show_exited(ui: &mut egui::Ui, process: &mut WatchedProcess) {
    if let Some(report) = process.poll() {
        ui.label(super::exit_line("mGBA", report));
    }
}

pub(super) fn exit_failure(
    report: &archivefs_core::launch::process_spawn::ProcessExitReport,
) -> Option<String> {
    match &report.status {
        Ok(status) if status.success() => None,
        _ => Some(super::exit_line("mGBA", report)),
    }
}

fn capture(path: &Path) -> Result<CapturedFileIdentity, String> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "{} is not a regular, non-symlink file",
            path.display()
        ));
    }
    Ok(CapturedFileIdentity::capture(&metadata))
}

fn fresh_identity(path: &Path, platform: &str) -> Result<CanonicalIdentityStatus, String> {
    let report = crate::selected_evidence_page::gather_selected_evidence_fast(path, Some(platform))
        .map_err(|e| format!("mGBA identity could not be rechecked: {e}"))?;
    let content = &report.identity_result.content;
    if matches!(
        content.outcome,
        FusionOutcome::Conflict | FusionOutcome::Ambiguous
    ) || content.resolved_platform.is_some_and(|p| p != platform)
    {
        return Err("Content platform evidence conflicts with the selected game. Rescan it before launching.".into());
    }
    Ok(archivefs_core::launch::canonical_identity_from_game_report(&report.game_identity_report).0)
}

fn review(
    plan: &LaunchPlan,
    candidate: &LaunchCandidate,
    roots: MgbaProfileDiscoveryRoots,
) -> Result<Reviewed, String> {
    Request::offer(plan, candidate)?;
    let LaunchTarget::Standalone {
        adapter_id: "mgba",
        profile_id,
        ..
    } = &candidate.target
    else {
        return Err("The selected candidate is not native mGBA.".into());
    };
    let path = candidate
        .content
        .resolved_path
        .as_ref()
        .ok_or("No direct cartridge path is available.")?;
    let platform = plan
        .platform_id
        .as_deref()
        .ok_or("Game platform is unresolved.")?;
    let game_key = plan
        .game_key
        .as_ref()
        .ok_or("Game identity is unresolved.")?;
    let profile = discover_mgba_profiles(&roots)
        .profiles
        .into_iter()
        .find(|p| &p.profile_id == profile_id)
        .ok_or("The selected mGBA profile is unavailable. Recheck emulator setup.")?;
    let binding = resolve_mgba_native_launch_binding(&profile).map_err(|e| e.detail)?;
    let (executable_identity, executable_sha256) = capture_mgba_executable(&binding.executable)
        .map_err(|e| format!("mGBA executable could not be verified: {e}"))?;
    let config = profile
        .config_path
        .as_deref()
        .map(capture_mgba_config)
        .transpose()
        .map_err(|e| format!("mGBA configuration could not be verified: {e}"))?;
    let core = MgbaLaunchRequest {
        selected_content_path: path.clone(),
        expected_platform_id: platform.into(),
        expected_game_key: game_key.clone(),
        profile_id: profile_id.clone(),
        expected_executable: binding.executable.clone(),
        content_identity: capture(path)?,
        executable_identity,
        executable_sha256,
        expected_config_path: profile.config_path.clone(),
        config_identity: config.map(|(identity, _)| identity),
        config_sha256: config.map(|(_, digest)| digest),
    };
    Ok(Reviewed { core, roots })
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use archivefs_core::launch::{
        CandidatePreference, FirmwareReadiness, LaunchContentRef, LaunchReadiness,
    };
    use std::collections::BTreeMap;

    fn ready_offer(plan: &LaunchPlan, candidate: &LaunchCandidate) -> Request {
        for _ in 0..100 {
            match Request::offer(plan, candidate) {
                Ok(request) => return request,
                Err(reason) if reason == CHECKING => std::thread::sleep(Duration::from_millis(20)),
                Err(reason) => panic!("unexpected refusal: {reason}"),
            }
        }
        panic!("Bubblewrap capability check did not complete")
    }

    fn fake_bubblewrap(path: &Path, supported: bool) {
        use std::os::unix::fs::PermissionsExt;
        let help = if supported {
            "--bind-fd --ro-bind-data --bind --dev-bind --proc --perms"
        } else {
            "--ro-bind-data --bind --dev-bind --proc --perms"
        };
        std::fs::write(
            path,
            format!("#!/bin/sh\n[ \"$1\" = --help ] || exit 91\nprintf '%s\\n' '{help}'\n"),
        )
        .unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn bubblewrap_capability_cache_refuses_missing_unsupported_and_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bwrap");
        let mut cache = ProbeCache::default();
        assert!(cache.offer(&path, true).unwrap_err().contains("required"));
        fake_bubblewrap(&path, false);
        assert_eq!(cache.offer(&path, true).unwrap_err(), CHECKING);
        for _ in 0..100 {
            match cache.offer(&path, true) {
                Err(reason) if reason == CHECKING => std::thread::sleep(Duration::from_millis(20)),
                Err(reason) => {
                    assert!(reason.contains("--bind-fd"));
                    break;
                }
                Ok(_) => panic!("unsupported Bubblewrap was offered"),
            }
        }
        let replacement = dir.path().join("replacement");
        fake_bubblewrap(&replacement, true);
        std::fs::rename(&replacement, &path).unwrap();
        assert_eq!(cache.offer(&path, true).unwrap_err(), CHECKING);
        let receipt = loop {
            match cache.offer(&path, true) {
                Ok(receipt) => break receipt,
                Err(reason) if reason == CHECKING => std::thread::sleep(Duration::from_millis(20)),
                Err(reason) => panic!("supported Bubblewrap refused: {reason}"),
            }
        };
        assert_eq!(probe_bubblewrap(&path, true).unwrap(), receipt);
        recheck_bubblewrap(&path, receipt, true).unwrap();
        let replacement = dir.path().join("replacement");
        fake_bubblewrap(&replacement, true);
        std::fs::rename(&replacement, &path).unwrap();
        assert!(
            recheck_bubblewrap(&path, receipt, true)
                .unwrap_err()
                .contains("changed")
        );
        fake_bubblewrap(&path, false);
        assert_ne!(probe_bubblewrap(&path, true).unwrap_err(), CHECKING);
        assert!(
            probe_bubblewrap(&path, true)
                .unwrap_err()
                .contains("--bind-fd")
        );
        assert!(probe_bubblewrap(&path, false).is_ok());
        std::fs::remove_file(&path).unwrap();
        assert!(cache.offer(&path, true).is_err());
    }

    #[test]
    fn bubblewrap_probe_retries_a_transient_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bwrap");
        fake_bubblewrap(&path, true);
        cache_retry_check(&path);
    }

    fn cache_retry_check(path: &Path) {
        let identity = bubblewrap_identity(path).unwrap();
        let mut cache = ProbeCache(Some(ProbeState::Complete(
            identity,
            true,
            Instant::now() - Duration::from_secs(4),
            Err("Bubblewrap capability check timed out. Recheck readiness.".into()),
        )));
        assert_eq!(cache.offer(path, true).unwrap_err(), CHECKING);
        for _ in 0..100 {
            match cache.offer(path, true) {
                Ok(_) => return,
                Err(reason) if reason == CHECKING => std::thread::sleep(Duration::from_millis(20)),
                Err(reason) => panic!("retry failed: {reason}"),
            }
        }
        panic!("Bubblewrap retry did not complete")
    }

    #[test]
    fn profile_config_controls_bind_fd_requirement() {
        let (dir, _roots, _plan, candidate) = fixture(0);
        assert!(!profile_uses_bind_fd(&candidate).unwrap());
        std::fs::write(dir.path().join("config/config.ini"), b"[mGBA]\n").unwrap();
        assert!(profile_uses_bind_fd(&candidate).unwrap());
    }

    fn fixture(
        code: i32,
    ) -> (
        tempfile::TempDir,
        MgbaProfileDiscoveryRoots,
        LaunchPlan,
        LaunchCandidate,
    ) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game.gba");
        std::fs::write(&game, b"synthetic-gba-cartridge-bytes").unwrap();
        let executable = dir.path().join("mgba");
        std::fs::write(&executable, format!("#!/bin/sh\nexit {code}\n")).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let config = dir.path().join("config");
        std::fs::create_dir(&config).unwrap();
        let roots = MgbaProfileDiscoveryRoots {
            home: dir.path().into(),
            xdg_config_home: dir.path().join("unused"),
            explicit_configuration_roots: vec![config.clone()],
            portable_configuration_roots: vec![],
            explicit_executables: vec![executable],
            known_version_outputs: BTreeMap::new(),
        };
        let identity = fresh_identity(&game, "Game Boy Advance").unwrap();
        let CanonicalIdentityStatus::Resolved(identity) = identity else {
            panic!("fixture identity unresolved")
        };
        let candidate = LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: "mgba",
                profile_id: format!("mgba:{}", config.display()),
                profile_path: None,
            },
            content: LaunchContentRef {
                kind: None,
                container: Some(LaunchContainerKind::PlainFile),
                resolved_path: Some(game),
                requires_mount: false,
                provenance: "test".into(),
            },
            firmware: FirmwareReadiness::NotRequired,
            blockers: vec![],
            warnings: vec![],
            readiness: LaunchReadiness::Ready,
            preference: CandidatePreference::SoleEligible,
        };
        let plan = LaunchPlan {
            platform_id: Some(identity.platform_id),
            game_key: Some(identity.game_key),
            candidates: vec![candidate.clone()],
            summary: Default::default(),
            media_topology: None,
        };
        (dir, roots, plan, candidate)
    }

    #[test]
    fn selected_cartridge_launches_and_nonzero_exit_is_visible() {
        for code in [0, 7] {
            let (_dir, roots, plan, candidate) = fixture(code);
            let mut process = ready_offer(&plan, &candidate)
                .execute_with_roots(roots)
                .unwrap();
            let report = loop {
                if let Some(report) = process.poll() {
                    break report;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            };
            assert_eq!(report.status.as_ref().unwrap().code(), Some(code));
            assert_eq!(exit_failure(report).is_some(), code != 0);
        }
    }

    #[test]
    fn changed_game_and_unsupported_archive_refuse() {
        let (_dir, roots, plan, candidate) = fixture(0);
        let request = ready_offer(&plan, &candidate);
        std::fs::write(
            candidate.content.resolved_path.as_ref().unwrap(),
            b"different bytes",
        )
        .unwrap();
        assert!(
            request
                .execute_with_roots(roots)
                .err()
                .unwrap()
                .contains("not started")
        );
        let (_dir, _roots, plan, mut candidate) = fixture(0);
        candidate.content.container = Some(LaunchContainerKind::Archive);
        assert!(Request::offer(&plan, &candidate).is_err());
    }

    #[test]
    fn selection_keys_do_not_reuse_an_old_games_result() {
        let (dir, _roots, plan, candidate) = fixture(0);
        let first = ready_offer(&plan, &candidate);
        let mut second_candidate = candidate.clone();
        let next = dir.path().join("next.gba");
        std::fs::write(&next, b"another synthetic game").unwrap();
        second_candidate.content.resolved_path = Some(next);
        let second = ready_offer(&plan, &second_candidate);
        assert_ne!(first.key(), second.key());
    }
}
