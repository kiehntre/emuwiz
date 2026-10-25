//! Explicit, offline, disposable WHDLoad boot templates for plain FS-UAE.
//! Authorization is the user's attestation, not inferred from file presence.
//! No downloads, inherited guest scripts/preferences, real saves or live mounts.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::amiga_whdload_command::WHDLoadLaunchInput;
use super::process_spawn::{CapturedFileIdentity, PreparedProcessCommand, WatchedProcess};
use super::safe_launch_sandbox::fs as workspace_fs;
use crate::dat::archive::{lha::LhaProvider, lha_header::LhaEntryKind, limits::ArchiveLimits};
use crate::identity_source::whdload::parse_whdload_slave;
use crate::safe_read::TrustedRoots;

const MAX_FILE: u64 = 32 * 1024 * 1024;
const MAX_TOTAL: u64 = 128 * 1024 * 1024;
const MAX_FILES: usize = 512;
const MAX_MANIFEST: u64 = 128 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TemplateFile {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BootManifest {
    pub schema: u32,
    pub machine: String,
    pub chip_memory_kib: u32,
    pub fast_memory_kib: u32,
    pub authorized_game: bool,
    pub authorized_template: bool,
    pub authorized_firmware: bool,
    pub disposable_saves: bool,
    pub provenance: String,
    pub game_key: String,
    pub package_sha256: String,
    pub slave_member: String,
    pub template_root: PathBuf,
    /// Version of the user-supplied WHDLoad; the supplied bytes are hash-bound.
    pub whdload_version: u16,
    /// Exactly Firmware/boot.rom plus explicitly supplied Boot/{C,Devs,...} files.
    pub files: Vec<TemplateFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Evidence {
    path: PathBuf,
    identity: CapturedFileIdentity,
    sha256: String,
    length: u64,
    changed: (i64, i64),
}

/// Opaque approval: callers cannot manufacture template/dependency evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootApproval {
    manifest: BootManifest,
    manifest_file: Evidence,
    package: Evidence,
    profile: Evidence,
    executable: Evidence,
    dependencies: Vec<Evidence>,
}

pub fn manifest_path(configuration: &Path) -> PathBuf {
    configuration.with_extension("whdload.json")
}

fn fail(message: impl Into<String>) -> String {
    format!("WHDLoad boot blocked: {}", message.into())
}

fn io(error: std::io::Error) -> String {
    fail(error.to_string())
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 240
        && path.split('/').count() <= 12
        && path.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part.len() <= 80
                && !part.to_ascii_lowercase().ends_with(".uaem")
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b" ._-".contains(&b))
                && !part.ends_with(' ')
                && !part.ends_with('.')
        })
}

fn hash_valid(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn read_open(path: &Path, limit: u64) -> Result<(File, Vec<u8>, Evidence), String> {
    let mut file = workspace_fs::absolute(path, false).map_err(io)?;
    let before = file.metadata().map_err(io)?;
    if !before.is_file() || before.len() > limit {
        return Err(fail("unsafe or oversized input"));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    let after = file.metadata().map_err(io)?;
    if bytes.len() as u64 != before.len()
        || CapturedFileIdentity::capture(&before) != CapturedFileIdentity::capture(&after)
        || (before.ctime(), before.ctime_nsec()) != (after.ctime(), after.ctime_nsec())
    {
        return Err(fail("input changed while reading"));
    }
    let evidence = Evidence {
        path: path.into(),
        identity: CapturedFileIdentity::capture(&before),
        sha256: digest(&bytes),
        length: before.len(),
        changed: (before.ctime(), before.ctime_nsec()),
    };
    Ok((file, bytes, evidence))
}

fn read(path: &Path, limit: u64) -> Result<(Vec<u8>, Evidence), String> {
    let (_, bytes, evidence) = read_open(path, limit)?;
    Ok((bytes, evidence))
}

/// Capture declared hashes and current file metadata without hashing payloads on the UI thread.
/// The private preparation path verifies these hashes before any emulator is started.
fn declared_evidence(path: &Path, sha256: &str, limit: u64) -> Result<Evidence, String> {
    let file = workspace_fs::absolute(path, false).map_err(io)?;
    let metadata = file.metadata().map_err(io)?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(fail("unsafe or oversized input"));
    }
    Ok(Evidence {
        path: path.into(),
        identity: CapturedFileIdentity::capture(&metadata),
        sha256: sha256.into(),
        length: metadata.len(),
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    })
}

fn revalidate(expected: &Evidence, limit: u64) -> Result<Vec<u8>, String> {
    let (bytes, current) = read(&expected.path, limit)?;
    if &current != expected {
        return Err(fail(format!("stale input: {}", expected.path.display())));
    }
    Ok(bytes)
}

fn path_spellings<'a>(paths: impl Iterator<Item = &'a str>) -> Result<(), String> {
    let mut spellings = BTreeMap::new();
    for path in paths {
        let mut prefix = String::new();
        for component in path.trim_end_matches('/').split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(component);
            if spellings
                .insert(prefix.to_ascii_lowercase(), prefix.clone())
                .is_some_and(|previous| previous != prefix)
            {
                return Err(fail("case-colliding directory spelling"));
            }
        }
    }
    Ok(())
}

fn template_files(manifest: &BootManifest) -> Result<BTreeSet<String>, String> {
    if manifest.files.is_empty() || manifest.files.len() > MAX_FILES {
        return Err(fail("template file limit"));
    }
    let mut names = BTreeSet::new();
    for file in &manifest.files {
        if !safe_relative(&file.path)
            || !hash_valid(&file.sha256)
            || !(file.path == "Firmware/boot.rom"
                || [
                    "Boot/C/",
                    "Boot/Devs/",
                    "Boot/Libs/",
                    "Boot/L/",
                    "Boot/Fonts/",
                ]
                .iter()
                .any(|p| file.path.starts_with(p)))
            || !names.insert(file.path.to_ascii_lowercase())
        {
            return Err(fail("unsafe, duplicate or unsupported template file"));
        }
    }
    for required in [
        "Firmware/boot.rom",
        "Boot/C/WHDLoad",
        "Boot/C/SetPatch",
        "Boot/C/uae-configuration",
    ] {
        if !manifest.files.iter().any(|f| f.path == required) {
            return Err(fail(format!("missing dependency {required}")));
        }
    }
    path_spellings(manifest.files.iter().map(|f| f.path.as_str()))?;
    Ok(names)
}

fn enumerate(
    directory: &File,
    prefix: &str,
    output: &mut BTreeSet<String>,
    depth: usize,
    visited: &mut usize,
) -> Result<(), String> {
    if depth > 12 {
        return Err(fail("template nesting limit"));
    }
    for entry in workspace_fs::entries(directory, MAX_FILES + 1).map_err(io)? {
        *visited += 1;
        if *visited > MAX_FILES * 2 {
            return Err(fail("template entry limit"));
        }
        let name = entry
            .to_str()
            .ok_or_else(|| fail("non-UTF8 template filename"))?;
        let path = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        if !safe_relative(&path) {
            return Err(fail("unsafe template path"));
        }
        let child = workspace_fs::child(directory, &entry, libc::O_RDONLY).map_err(io)?;
        let metadata = child.metadata().map_err(io)?;
        if metadata.is_dir() {
            enumerate(&child, &path, output, depth + 1, visited)?;
        } else if !metadata.is_file() || !output.insert(path) {
            return Err(fail("unsupported template entry"));
        }
        if output.len() > MAX_FILES {
            return Err(fail("template file limit"));
        }
    }
    Ok(())
}

/// Read-only approval from the explicitly supplied per-profile sidecar.
/// The JSON must attest authorization; mere discovery never authorizes launch.
pub fn inspect_boot(request: &WHDLoadLaunchInput) -> Result<BootApproval, String> {
    use super::amiga_whdload_command::WhdloadPackageFormat;
    use super::planning::CanonicalIdentityStatus;
    use crate::patch_manager::AmigaEmulatorKind;
    let CanonicalIdentityStatus::Resolved(identity) = &request.identity else {
        return Err(fail("verified identity required"));
    };
    if identity.platform_id != "Amiga"
        || identity.game_key.trim().is_empty()
        || request.profile.profile_id.trim().is_empty()
        || identity.game_key != request.profile.verified_identity
        || request.profile.emulator != AmigaEmulatorKind::FsUae
        || request.profile.candidate_count != 1
        || !request.profile.eligible
    {
        return Err(fail("ambiguous or mismatched profile/identity"));
    }
    let target = request
        .target
        .as_ref()
        .ok_or_else(|| fail("missing package"))?;
    if target.format != WhdloadPackageFormat::Lha
        || !target
            .package_path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("lha") || s.eq_ignore_ascii_case("lzh"))
    {
        return Err(fail("only verified LHA/LZH packages are supported"));
    }
    let configuration = request
        .profile
        .configuration
        .as_ref()
        .ok_or_else(|| fail("missing profile"))?;
    let executable = request
        .profile
        .executable
        .as_ref()
        .ok_or_else(|| fail("missing FS-UAE executable"))?;
    let (bytes, manifest_file) =
        read(&manifest_path(configuration), MAX_MANIFEST).map_err(|e| {
            fail(format!(
                "supply explicit authorized template manifest {}: {e}",
                manifest_path(configuration).display()
            ))
        })?;
    let manifest: BootManifest = serde_json::from_slice(&bytes).map_err(|e| fail(e.to_string()))?;
    if manifest.schema != 1
        || manifest.machine != "A1200"
        || manifest.chip_memory_kib != 2048
        || manifest.fast_memory_kib != 8192
        || !manifest.authorized_game
        || !manifest.authorized_template
        || !manifest.authorized_firmware
        || !manifest.disposable_saves
        || manifest.provenance.trim().is_empty()
        || manifest.provenance.len() > 2048
        || manifest.game_key != identity.game_key
        || !hash_valid(&manifest.package_sha256)
        || !safe_relative(&manifest.slave_member)
        || !manifest
            .slave_member
            .to_ascii_lowercase()
            .ends_with(".slave")
        || manifest.whdload_version != 18
    {
        return Err(fail(
            "authorization, game binding, WHDLoad version 18 or disposable-save acknowledgement missing/unsupported",
        ));
    }
    template_files(&manifest)?;
    let mut actual = BTreeSet::new();
    enumerate(
        &workspace_fs::absolute(&manifest.template_root, true).map_err(io)?,
        "",
        &mut actual,
        0,
        &mut 0,
    )?;
    let declared = manifest
        .files
        .iter()
        .map(|f| f.path.clone())
        .collect::<BTreeSet<_>>();
    if actual != declared {
        return Err(fail("template has missing or unlisted files"));
    }
    let mut total = 0u64;
    let mut dependencies = Vec::new();
    for spec in &manifest.files {
        let evidence = declared_evidence(
            &manifest.template_root.join(&spec.path),
            &spec.sha256,
            MAX_FILE,
        )?;
        total += evidence.length;
        if total > MAX_TOTAL || evidence.length == 0 {
            return Err(fail("template dependency hash/size mismatch"));
        }
        if spec.path == "Firmware/boot.rom" && evidence.length != 512 * 1024 {
            return Err(fail("A1200 boot ROM must be 512 KiB"));
        }
        dependencies.push(evidence);
    }
    let package = declared_evidence(&target.package_path, &manifest.package_sha256, MAX_TOTAL)?;
    let (_, profile) = read(configuration, MAX_MANIFEST)?;
    let (_, executable) = read(executable, MAX_FILE)?;
    Ok(BootApproval {
        manifest,
        manifest_file,
        package,
        profile,
        executable,
        dependencies,
    })
}

fn write_new(root: &File, relative: &str, bytes: &[u8]) -> Result<(), String> {
    if !safe_relative(relative) {
        return Err(fail("unsafe staging destination"));
    }
    let parts: Vec<_> = relative.split('/').collect();
    let mut directory = root.try_clone().map_err(io)?;
    for part in &parts[..parts.len() - 1] {
        directory = workspace_fs::mkdir(&directory, part.as_ref(), true).map_err(io)?;
    }
    let mut file = workspace_fs::create(&directory, parts.last().unwrap().as_ref()).map_err(io)?;
    file.write_all(bytes).map_err(io)?;
    file.sync_all().map_err(io)
}

/// Owns all guest-writable files. No real content path appears in its config.
pub struct PreparedBoot {
    workspace: tempfile::TempDir,
    command: PreparedProcessCommand,
    environment: Vec<(OsString, OsString)>,
    executable: Evidence,
}

impl PreparedBoot {
    pub fn workspace_path(&self) -> &Path {
        self.workspace.path()
    }
    pub fn command(&self) -> &PreparedProcessCommand {
        &self.command
    }
    pub(crate) fn spawn(self) -> Result<(PreparedProcessCommand, WatchedProcess), String> {
        // Re-open without following ancestors/links, hash and retain this exact
        // native ELF descriptor through exec. A path replacement after checking
        // cannot select a different binary. CLOEXEC prevents leaking it to FS-UAE.
        let (executable, bytes, evidence) = read_open(&self.executable.path, MAX_FILE)?;
        if evidence != self.executable
            || self.command.executable != self.executable.path
            || bytes.get(..4) != Some(b"\x7fELF")
        {
            return Err(fail(
                "FS-UAE executable changed or is not a native ELF binary",
            ));
        }
        let command = self.command;
        let workspace = self.workspace;
        let watched = super::process_spawn::spawn_watched_process_private_pinned(
            &command,
            &self.environment,
            &executable,
            move |_| drop(workspace),
        )
        .map_err(io)?;
        drop(executable);
        Ok((command, watched))
    }
}

/// Revalidates approval, copies only into a private workspace, and decodes the
/// private package using the existing bounded LHA backend. Never mounts originals.
pub fn prepare_boot(
    request: &WHDLoadLaunchInput,
    approval: &BootApproval,
) -> Result<PreparedBoot, String> {
    let fresh = inspect_boot(request)?;
    if &fresh != approval {
        return Err(fail("boot approval is stale; review readiness again"));
    }
    let workspace = tempfile::Builder::new()
        .prefix("emuwiz-whdload-")
        .tempdir()
        .map_err(io)?;
    let root = workspace_fs::absolute(workspace.path(), true).map_err(io)?;
    let capacity = crate::diagnostics::environment::filesystem_stat(workspace.path())
        .ok_or_else(|| fail("cannot determine disposable staging capacity"))?;
    if capacity.available_bytes < MAX_TOTAL * 4 {
        return Err(fail("insufficient disposable staging capacity"));
    }
    let package_bytes = revalidate(&approval.package, MAX_TOTAL)?;
    write_new(&root, "package.lha", &package_bytes)?;
    for (spec, expected) in approval.manifest.files.iter().zip(&approval.dependencies) {
        let bytes = revalidate(expected, MAX_FILE)?;
        if [
            "Boot/C/WHDLoad",
            "Boot/C/SetPatch",
            "Boot/C/uae-configuration",
        ]
        .contains(&spec.path.as_str())
            && bytes.get(..4) != Some(&[0, 0, 3, 0xf3])
        {
            return Err(fail("required guest tool is not an Amiga HUNK executable"));
        }
        if spec.path == "Boot/C/WHDLoad"
            && !bytes
                .windows(b"$VER: WHDLoad 18.".len())
                .any(|w| w == b"$VER: WHDLoad 18.")
        {
            return Err(fail(
                "supplied WHDLoad does not declare supported version 18",
            ));
        }
        write_new(&root, &spec.path, &bytes)?;
    }
    let provider =
        LhaProvider::discover(Duration::from_secs(5)).map_err(|e| fail(e.to_string()))?;
    let archive = provider
        .open(
            &workspace.path().join("package.lha"),
            &TrustedRoots::none(),
            ArchiveLimits {
                max_members: MAX_FILES,
                max_member_logical_bytes: MAX_FILE,
                max_archive_logical_bytes: MAX_TOTAL,
                ..Default::default()
            },
            Duration::from_secs(30),
        )
        .map_err(|e| fail(e.to_string()))?;
    let members: Vec<_> = archive.member_infos().collect();
    path_spellings(members.iter().map(|m| m.path))?;
    let mut names = BTreeMap::new();
    let mut total = 0u64;
    for member in &members {
        let path = member.path.trim_end_matches('/');
        if !safe_relative(path)
            || names
                .insert(path.to_ascii_lowercase(), member.kind.clone())
                .is_some()
        {
            return Err(fail("unsafe or case-colliding package path"));
        }
        if !matches!(member.kind, LhaEntryKind::Regular | LhaEntryKind::Directory) {
            return Err(fail("unsupported archive entry type"));
        }
        total = total
            .checked_add(member.logical_size)
            .ok_or_else(|| fail("archive size overflow"))?;
        if total > MAX_TOTAL || member.logical_size > MAX_FILE {
            return Err(fail("package staging limit"));
        }
    }
    for path in names.keys() {
        let mut parent = Path::new(path).parent();
        while let Some(p) = parent.filter(|p| !p.as_os_str().is_empty()) {
            if names
                .get(p.to_str().unwrap())
                .is_some_and(|k| *k != LhaEntryKind::Directory)
            {
                return Err(fail("file/directory prefix collision"));
            }
            parent = p.parent();
        }
    }
    let cancel = AtomicBool::new(false);
    let mut slaves = Vec::new();
    for member in members {
        if matches!(member.kind, LhaEntryKind::Directory) {
            let relative = format!("Game/{}", member.path.trim_end_matches('/'));
            let mut directory = root.try_clone().map_err(io)?;
            for component in relative.split('/') {
                directory =
                    workspace_fs::mkdir(&directory, component.as_ref(), true).map_err(io)?;
            }
            continue;
        }
        let bytes = archive
            .read_member(member.path, MAX_FILE, &cancel)
            .map_err(|e| fail(e.to_string()))?;
        if member.path.to_ascii_lowercase().ends_with(".slave") {
            let parsed = parse_whdload_slave(&bytes).map_err(|e| fail(e.to_string()))?;
            slaves.push((member.path.to_owned(), parsed, digest(&bytes)));
        }
        write_new(&root, &format!("Game/{}", member.path), &bytes)?;
    }
    let [(path, slave, slave_hash)] = slaves.as_slice() else {
        return Err(fail("exactly one valid slave is required"));
    };
    if path != &approval.manifest.slave_member
        || request.slave.as_ref().is_none_or(|s| {
            s.parsed != *slave
                || s.sha256 != *slave_hash
                || Path::new(path).file_name().and_then(|n| n.to_str()) != Some(s.name.as_str())
        })
    {
        return Err(fail("selected slave evidence changed"));
    }
    if slave.runtime_version > approval.manifest.whdload_version
        || slave.base_mem_size > 2 * 1024 * 1024
        || slave
            .exp_mem
            .is_some_and(|n| n & 0x8000_0000 != 0 || n > 8 * 1024 * 1024)
    {
        return Err(fail("unsupported slave memory/runtime requirements"));
    }
    if let Some(kick) = &slave.kick_name {
        if !safe_relative(kick) || kick.contains('/') {
            return Err(fail("unsupported Kickstart name"));
        }
        let spec = format!("Boot/Devs/Kickstarts/{kick}");
        let rom = approval
            .manifest
            .files
            .iter()
            .position(|f| f.path == spec)
            .ok_or_else(|| fail("missing slave Kickstart"))?;
        if slave
            .kick_size
            .is_none_or(|n| n as u64 != approval.dependencies[rom].length)
            || !approval
                .manifest
                .files
                .iter()
                .any(|f| f.path == format!("{spec}.RTB"))
        {
            return Err(fail("missing/wrong slave Kickstart or RTB"));
        }
        let rom_bytes = revalidate(&approval.dependencies[rom], MAX_FILE)?;
        let mut crc = crate::dat::archive::lha_header::Crc16::default();
        crc.update(&rom_bytes);
        if slave.kick_crc != Some(crc.finish()) {
            return Err(fail("slave Kickstart CRC mismatch"));
        }
    }
    let parent = Path::new(path)
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or("");
    let name = Path::new(path).file_name().unwrap().to_str().unwrap();
    // Safe path grammar forbids Amiga shell quotes, colon, escapes and controls.
    let sequence = format!(
        "C:SetPatch QUIET\nCD \"Games:{parent}\"\nC:WHDLoad \"{name}\" PRELOAD\nC:uae-configuration SPC_QUIT 1\n"
    );
    write_new(&root, "Boot/S/Startup-Sequence", sequence.as_bytes())?;
    let path_text = workspace
        .path()
        .to_str()
        .ok_or_else(|| fail("non-UTF8 workspace"))?;
    if !path_text
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"/ ._-".contains(&b))
    {
        return Err(fail("unsupported workspace path"));
    }
    let config = format!(
        "[fs-uae]\namiga_model = A1200\nchip_memory = 2048\nfast_memory = 8192\nkickstart_file = {path_text}/Firmware/boot.rom\nhard_drive_0 = {path_text}/Boot\nhard_drive_0_label = System\nhard_drive_0_priority = 10\nhard_drive_0_read_only = 1\nhard_drive_1 = {path_text}/Game\nhard_drive_1_label = Games\nhard_drive_1_priority = -128\nhard_drive_1_read_only = 0\nbase_dir = {path_text}/home/FS-UAE\nstate_dir = {path_text}/state\njoystick_port_1 = keyboard\n"
    );
    write_new(&root, "session.fs-uae", config.as_bytes())?;
    let mut environment = vec![("PATH".into(), "/usr/bin:/bin".into())];
    for (key, folder) in [
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_STATE_HOME", "state"),
        ("TMPDIR", "tmp"),
    ] {
        workspace_fs::mkdir(&root, folder.as_ref(), true).map_err(io)?;
        environment.push((key.into(), workspace.path().join(folder).into_os_string()));
    }
    for key in [
        "DISPLAY",
        "XAUTHORITY",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
        "WAYLAND_DISPLAY",
        "PULSE_SERVER",
    ] {
        if let Some(value) = std::env::var_os(key) {
            environment.push((key.into(), value));
        }
    }
    // Final source revalidation occurs after potentially slow decoding.
    if inspect_boot(request)? != *approval {
        return Err(fail("inputs changed during preparation"));
    }
    let command = PreparedProcessCommand {
        executable: approval.executable.path.clone(),
        arguments: vec![workspace.path().join("session.fs-uae").into_os_string()],
        working_directory: Some(workspace.path().into()),
    };
    Ok(PreparedBoot {
        workspace,
        command,
        environment,
        executable: approval.executable.clone(),
    })
}

#[cfg(test)]
mod tests;

// Pure argv-planner tests cannot authorize a real launch. This value has no
// real manifest/dependency paths, so actual preparation always refuses it.
#[cfg(test)]
pub(crate) fn preview_fixture() -> BootApproval {
    let evidence = Evidence {
        path: "/nonexistent/job38-preview-only".into(),
        identity: CapturedFileIdentity {
            device: 0,
            inode: 0,
            size: 0,
            modified: None,
        },
        sha256: "00".repeat(32),
        length: 0,
        changed: (0, 0),
    };
    BootApproval {
        manifest: BootManifest {
            schema: 1,
            machine: "A1200".into(),
            chip_memory_kib: 2048,
            fast_memory_kib: 8192,
            authorized_game: false,
            authorized_template: false,
            authorized_firmware: false,
            disposable_saves: false,
            provenance: "pure planner fixture, not launch authorization".into(),
            game_key: "game".into(),
            package_sha256: "00".repeat(32),
            slave_member: "Game.slave".into(),
            template_root: "/nonexistent/job38-preview-only".into(),
            whdload_version: 18,
            files: vec![],
        },
        manifest_file: evidence.clone(),
        package: evidence.clone(),
        profile: evidence.clone(),
        executable: evidence,
        dependencies: vec![],
    }
}
