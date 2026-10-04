//! Chooses a per-launch scratch root the launched RetroArch can actually see.
//!
//! A Flatpak RetroArch has a private `/tmp`, so the default
//! `<tmp>/emuwiz/retroarch-launches` root is invisible to it: RetroArch logs
//! `Config not found`. A user may also have `flatpak run` wrapped in a script
//! (which discovery classifies as a native executable). This module therefore
//! classifies the executable that will really be spawned and, for Flatpak,
//! proves from the installed app's metadata plus Flatpak overrides that the
//! EmuWiz-owned root (`<EmuWiz data dir>/retroarch-launches`) is granted
//! read/write. Visibility that cannot be established fails closed; nothing
//! here executes a process or writes a file.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::launch::retroarch_resource_projection::approved_retroarch_launch_root;

/// How the executable that will be spawned reaches RetroArch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetroArchSandbox {
    /// A native program or an ordinary wrapper: the host `/tmp` is visible.
    None,
    /// `flatpak run <app id>` (directly or through a wrapper script).
    Flatpak { app_id: String },
    /// A sandbox/container wrapper (or an unreadable executable) whose
    /// filesystem view cannot be determined.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetroArchRootError {
    SandboxUnknown,
    /// No Flatpak metadata for the app could be read.
    FlatpakPermissionsUnavailable(String),
    /// The sandbox does not grant read/write access to the root.
    NotVisible(PathBuf),
    /// A resource the launched RetroArch needs cannot be shown to be reachable.
    ResourceNotVisible {
        resource: &'static str,
        path: PathBuf,
    },
    DataDirUnavailable,
}

impl std::fmt::Display for RetroArchRootError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SandboxUnknown => {
                write!(f, "cannot tell what filesystem view RetroArch will have")
            }
            Self::FlatpakPermissionsUnavailable(app) => {
                write!(f, "no readable Flatpak permissions for {app}")
            }
            Self::NotVisible(path) => write!(
                f,
                "the Flatpak sandbox is not granted read/write access to {}",
                path.display()
            ),
            Self::ResourceNotVisible { resource, path } => write!(
                f,
                "the Flatpak sandbox cannot be shown to reach the {resource} ({}); \
                 nothing was started or written",
                path.display()
            ),
            Self::DataDirUnavailable => write!(f, "the EmuWiz data directory is unavailable"),
        }
    }
}

impl std::error::Error for RetroArchRootError {}

const MAX_SCRIPT_BYTES: u64 = 16 * 1024;
const SANDBOX_WORDS: &[&str] = &["flatpak", "snap", "bwrap", "firejail", "docker", "podman"];

/// Classifies the executable by content: ELF is native; a script that runs
/// `flatpak run <id>` is Flatpak; a script mentioning another sandbox tool
/// (or an unreadable file) is Unknown; any other script is a plain wrapper.
pub fn classify_retroarch_executable(executable: &Path) -> RetroArchSandbox {
    let Ok(file) = fs::File::open(executable) else {
        return RetroArchSandbox::Unknown;
    };
    let mut bytes = Vec::new();
    if file.take(MAX_SCRIPT_BYTES).read_to_end(&mut bytes).is_err() {
        return RetroArchSandbox::Unknown;
    }
    if bytes.starts_with(b"\x7fELF") || !bytes.starts_with(b"#!") {
        return RetroArchSandbox::None;
    }
    let text = String::from_utf8_lossy(&bytes);
    let words: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c == ';' || c == '&' || c == '|')
        .map(|word| word.trim_matches(|c| c == '"' || c == '\''))
        .filter(|word| !word.is_empty())
        .collect();
    for (index, word) in words.iter().enumerate() {
        if word.rsplit('/').next() == Some("flatpak") && words.get(index + 1) == Some(&"run") {
            let app = words[index + 2..]
                .iter()
                .find(|candidate| !candidate.starts_with('-'));
            return match app {
                Some(app) if app.contains('.') && !app.contains(['$', '/']) => {
                    RetroArchSandbox::Flatpak {
                        app_id: (*app).to_string(),
                    }
                }
                _ => RetroArchSandbox::Unknown,
            };
        }
    }
    if words
        .iter()
        .any(|word| SANDBOX_WORDS.contains(&word.rsplit('/').next().unwrap_or("")))
    {
        return RetroArchSandbox::Unknown;
    }
    RetroArchSandbox::None
}

/// Where Flatpak's own files and the user's directories live.
#[derive(Debug, Clone)]
pub struct FlatpakHost {
    pub home: PathBuf,
    pub xdg_data_home: PathBuf,
    pub xdg_cache_home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub system_flatpak_root: PathBuf,
}

impl FlatpakHost {
    pub fn from_env() -> Option<Self> {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        let xdg = |name: &str, fallback: &str| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .unwrap_or_else(|| home.join(fallback))
        };
        Some(Self {
            xdg_data_home: xdg("XDG_DATA_HOME", ".local/share"),
            xdg_cache_home: xdg("XDG_CACHE_HOME", ".cache"),
            xdg_config_home: xdg("XDG_CONFIG_HOME", ".config"),
            system_flatpak_root: PathBuf::from("/var/lib/flatpak"),
            home,
        })
    }
}

/// Effective `[Context] filesystems` entries, in Flatpak precedence order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FlatpakFilesystems {
    entries: Vec<String>,
    /// `~/.var/app/<app id>`: the sandbox's own config/data/cache, which it
    /// always has regardless of any `filesystems` entry.
    app_home: Option<PathBuf>,
}

fn context_filesystems(text: &str) -> Vec<String> {
    let mut in_context = false;
    let mut out = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_context = line == "[Context]";
        } else if in_context && let Some(value) = line.strip_prefix("filesystems=") {
            out.extend(
                value
                    .split(';')
                    .filter(|entry| !entry.is_empty())
                    .map(str::to_string),
            );
        }
    }
    out
}

impl FlatpakFilesystems {
    /// Applies metadata then overrides in order; `!entry` removes an entry.
    pub fn from_layers<'a>(layers: impl IntoIterator<Item = &'a str>) -> Self {
        let mut entries: Vec<String> = Vec::new();
        for layer in layers {
            for entry in context_filesystems(layer) {
                if let Some(negated) = entry.strip_prefix('!') {
                    entries.retain(|existing| strip_mode(existing).0 != strip_mode(negated).0);
                } else {
                    entries.retain(|existing| strip_mode(existing).0 != strip_mode(&entry).0);
                    entries.push(entry);
                }
            }
        }
        Self {
            entries,
            app_home: None,
        }
    }

    /// Loads the installed app's metadata and the system/user, global/app
    /// overrides. `None` when no metadata for the app can be read.
    pub fn load(host: &FlatpakHost, app_id: &str) -> Option<Self> {
        let user_root = host.xdg_data_home.join("flatpak");
        let system = &host.system_flatpak_root;
        let metadata = [&user_root, system].iter().find_map(|root| {
            fs::read_to_string(
                root.join("app")
                    .join(app_id)
                    .join("current/active/metadata"),
            )
            .ok()
        })?;
        let mut layers = vec![metadata];
        for path in [
            system.join("overrides/global"),
            system.join("overrides").join(app_id),
            user_root.join("overrides/global"),
            user_root.join("overrides").join(app_id),
        ] {
            if let Ok(text) = fs::read_to_string(path) {
                layers.push(text);
            }
        }
        let mut loaded = Self::from_layers(layers.iter().map(String::as_str));
        loaded.app_home = Some(host.home.join(".var/app").join(app_id));
        Some(loaded)
    }

    /// True only when an entry grants read/write (not `:ro`) access to `path`.
    pub fn grants_read_write(&self, host: &FlatpakHost, path: &Path) -> bool {
        self.grants(host, path, true)
    }

    /// True when `path` is readable inside the sandbox (a `:ro` grant counts).
    pub fn grants_read(&self, host: &FlatpakHost, path: &Path) -> bool {
        self.grants(host, path, false)
    }

    fn grants(&self, host: &FlatpakHost, path: &Path, write: bool) -> bool {
        const RESERVED: &[&str] = &[
            "/app", "/bin", "/dev", "/etc", "/lib", "/lib32", "/lib64", "/proc", "/run", "/sbin",
            "/sys", "/tmp", "/usr", "/var",
        ];
        if !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return false;
        }
        let reserved = |path: &Path| {
            !path.starts_with(&host.home) && RESERVED.iter().any(|r| path.starts_with(r))
        };
        if self
            .app_home
            .as_ref()
            .is_some_and(|own| path.starts_with(own))
        {
            return true;
        }
        self.entries.iter().any(|entry| {
            let (name, mode) = strip_mode(entry);
            if write && mode == "ro" {
                return false;
            }
            match name {
                "host" => !reserved(path),
                "home" => path.starts_with(&host.home),
                _ => {
                    let base = if let Some(rest) = name.strip_prefix("~/") {
                        Some(host.home.join(rest))
                    } else if name.starts_with('/') {
                        Some(PathBuf::from(name))
                    } else {
                        [
                            ("xdg-data", &host.xdg_data_home),
                            ("xdg-cache", &host.xdg_cache_home),
                            ("xdg-config", &host.xdg_config_home),
                        ]
                        .iter()
                        .find_map(|(prefix, dir)| {
                            if name == *prefix {
                                Some((*dir).clone())
                            } else {
                                name.strip_prefix(&format!("{prefix}/"))
                                    .map(|rest| dir.join(rest))
                            }
                        })
                    };
                    base.is_some_and(|base| path.starts_with(&base) && !reserved(&base))
                }
            }
        })
    }
}

fn strip_mode(entry: &str) -> (&str, &str) {
    match entry.rsplit_once(':') {
        Some((name, mode)) if matches!(mode, "ro" | "rw" | "create") => (name, mode),
        _ => (entry, "rw"),
    }
}

/// Everything the spawned RetroArch itself opens for one cheat launch.
#[derive(Debug, Clone)]
pub struct RetroArchLaunchResources<'a> {
    /// The launch-owned workspace: generated config and cheat derivative.
    pub workspace: &'a Path,
    pub content: &'a Path,
    pub core: Option<&'a Path>,
    pub saves: &'a Path,
    pub states: &'a Path,
}

/// Proves that RetroArch, launched via `executable`, can reach every resource
/// in `needs`; otherwise names the first one it cannot. A native RetroArch
/// shares the host filesystem. An unknown sandbox is refused. For Flatpak the
/// proof comes from the installed app's metadata and overrides (plus the
/// app's own `~/.var/app/<id>` directory); the workspace, saves and states
/// must be writable, the content and core readable. Nothing is relocated.
pub fn ensure_retroarch_resources_visible(
    executable: &Path,
    needs: &RetroArchLaunchResources<'_>,
    host: Option<&FlatpakHost>,
) -> Result<(), RetroArchRootError> {
    match classify_retroarch_executable(executable) {
        RetroArchSandbox::None => Ok(()),
        RetroArchSandbox::Unknown => Err(RetroArchRootError::SandboxUnknown),
        RetroArchSandbox::Flatpak { app_id } => {
            let host = host
                .ok_or_else(|| RetroArchRootError::FlatpakPermissionsUnavailable(app_id.clone()))?;
            let filesystems = FlatpakFilesystems::load(host, &app_id)
                .ok_or(RetroArchRootError::FlatpakPermissionsUnavailable(app_id))?;
            let checks: [(&'static str, Option<&Path>, bool); 5] = [
                ("launch workspace", Some(needs.workspace), true),
                ("game content", Some(needs.content), false),
                ("emulator core", needs.core, false),
                ("save directory", Some(needs.saves), true),
                ("save-state directory", Some(needs.states), true),
            ];
            for (resource, path, write) in checks {
                let Some(path) = path else { continue };
                let reachable = if write {
                    filesystems.grants_read_write(host, path)
                } else {
                    filesystems.grants_read(host, path)
                };
                if !reachable {
                    return Err(RetroArchRootError::ResourceNotVisible {
                        resource,
                        path: path.to_path_buf(),
                    });
                }
            }
            Ok(())
        }
    }
}

/// The directory under which a launch's cheat workspace is created: the
/// existing temporary root for a native RetroArch, otherwise the EmuWiz-owned
/// data-directory root (a Flatpak RetroArch has a private `/tmp`). Pure path
/// selection; creates nothing and proves nothing - the resource check does.
pub fn select_retroarch_approved_root(
    executable: &Path,
    data_launch_root: Option<&Path>,
) -> Result<PathBuf, RetroArchRootError> {
    match classify_retroarch_executable(executable) {
        RetroArchSandbox::None => Ok(approved_retroarch_launch_root()),
        _ => data_launch_root
            .map(Path::to_path_buf)
            .ok_or(RetroArchRootError::DataDirUnavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Layout {
        temp: tempfile::TempDir,
        host: FlatpakHost,
    }

    fn layout() -> Layout {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let host = FlatpakHost {
            xdg_data_home: home.join(".local/share"),
            xdg_cache_home: home.join(".cache"),
            xdg_config_home: home.join(".config"),
            system_flatpak_root: temp.path().join("system-flatpak"),
            home,
        };
        Layout { temp, host }
    }

    impl Layout {
        fn install(&self, app: &str, filesystems: &str) {
            let dir = self
                .host
                .xdg_data_home
                .join("flatpak/app")
                .join(app)
                .join("current/active");
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("metadata"),
                format!("[Application]\nname={app}\n\n[Context]\nfilesystems={filesystems}\n"),
            )
            .unwrap();
        }
        fn user_override(&self, name: &str, filesystems: &str) {
            let dir = self.host.xdg_data_home.join("flatpak/overrides");
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join(name),
                format!("[Context]\nfilesystems={filesystems}\n"),
            )
            .unwrap();
        }
        fn script(&self, name: &str, body: &str) -> PathBuf {
            let path = self.temp.path().join(name);
            fs::write(&path, body).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        }
        fn data_root(&self) -> PathBuf {
            self.host.xdg_data_home.join("emuwiz/retroarch-launches")
        }
        fn flatpak_wrapper(&self) -> PathBuf {
            self.script(
                "retroarch",
                "#!/usr/bin/env bash\nexec flatpak run \"org.libretro.RetroArch\" \"$@\"\n",
            )
        }
    }

    const APP: &str = "org.libretro.RetroArch";

    #[test]
    fn executables_are_classified_by_content() {
        let l = layout();
        assert_eq!(
            classify_retroarch_executable(&l.flatpak_wrapper()),
            RetroArchSandbox::Flatpak { app_id: APP.into() }
        );
        let elf = l.temp.path().join("elf");
        fs::write(&elf, b"\x7fELF....").unwrap();
        assert_eq!(classify_retroarch_executable(&elf), RetroArchSandbox::None);
        assert_eq!(
            classify_retroarch_executable(
                &l.script("plain", "#!/bin/sh\nexec /opt/ra/retroarch \"$@\"\n")
            ),
            RetroArchSandbox::None
        );
        let opts = l.script("opts", "#!/bin/sh\nexec /usr/bin/flatpak run --branch=stable --user org.libretro.RetroArch \"$@\"\n");
        assert_eq!(
            classify_retroarch_executable(&opts),
            RetroArchSandbox::Flatpak { app_id: APP.into() }
        );
        for body in [
            "#!/bin/sh\nexec bwrap --ro-bind / / retroarch\n",
            "#!/bin/sh\nexec flatpak run \"$APP\"\n",
        ] {
            assert_eq!(
                classify_retroarch_executable(&l.script("odd", body)),
                RetroArchSandbox::Unknown
            );
        }
        assert_eq!(
            classify_retroarch_executable(&l.temp.path().join("missing")),
            RetroArchSandbox::Unknown
        );
    }

    /// Resources that are always reachable (the app's own profile), so a test
    /// can vary only the one thing it is about.
    struct Needs {
        own: PathBuf,
        workspace: PathBuf,
        content: PathBuf,
        core: PathBuf,
        saves: PathBuf,
        states: PathBuf,
    }

    impl Needs {
        fn new(l: &Layout, workspace: PathBuf) -> Self {
            let own = l
                .host
                .home
                .join(".var/app")
                .join(APP)
                .join("config/retroarch");
            Needs {
                workspace,
                content: own.join("content/game.md"),
                core: own.join("cores/core_libretro.so"),
                saves: own.join("saves"),
                states: own.join("states"),
                own,
            }
        }
        fn check(&self, exe: &Path, host: Option<&FlatpakHost>) -> Result<(), RetroArchRootError> {
            ensure_retroarch_resources_visible(
                exe,
                &RetroArchLaunchResources {
                    workspace: &self.workspace,
                    content: &self.content,
                    core: Some(&self.core),
                    saves: &self.saves,
                    states: &self.states,
                },
                host,
            )
        }
    }

    fn refused(error: Result<(), RetroArchRootError>) -> &'static str {
        match error {
            Err(RetroArchRootError::ResourceNotVisible { resource, .. }) => resource,
            other => panic!("expected a refused resource, got {other:?}"),
        }
    }

    #[test]
    fn native_keeps_the_temporary_root_and_needs_no_proof() {
        let l = layout();
        let plain = l.script("plain", "#!/bin/sh\nexit 0\n");
        assert_eq!(
            select_retroarch_approved_root(&plain, None).unwrap(),
            approved_retroarch_launch_root()
        );
        let n = Needs::new(&l, approved_retroarch_launch_root().join("cheats-x"));
        assert!(n.check(&plain, None).is_ok());
    }

    #[test]
    fn flatpak_selects_the_emuwiz_data_root_never_the_private_tmp() {
        let l = layout();
        l.install(APP, "host;");
        let wrapper = l.flatpak_wrapper();
        let data = l.data_root();
        assert_eq!(
            select_retroarch_approved_root(&wrapper, Some(&data)).unwrap(),
            data
        );
        assert!(
            Needs::new(&l, data.join("cheats-id"))
                .check(&wrapper, Some(&l.host))
                .is_ok()
        );
        // Even with `host`, the temporary root is not visible to the sandbox.
        let tmp = Needs::new(&l, approved_retroarch_launch_root().join("cheats-x"));
        assert_eq!(
            refused(tmp.check(&wrapper, Some(&l.host))),
            "launch workspace"
        );
    }

    #[test]
    fn workspace_visibility_is_proven_from_grants_and_fails_closed_otherwise() {
        let l = layout();
        let wrapper = l.flatpak_wrapper();
        let n = Needs::new(&l, l.data_root().join("cheats-id"));
        let pick = |l: &Layout| n.check(&wrapper, Some(&l.host));
        // No metadata at all.
        assert!(matches!(
            pick(&l),
            Err(RetroArchRootError::FlatpakPermissionsUnavailable(_))
        ));
        // No host-side grant that covers the data root.
        l.install(APP, "xdg-run/pipewire-0;xdg-config/kdeglobals:ro;");
        assert_eq!(refused(pick(&l)), "launch workspace");
        // Read-only grant is not enough: RetroArch writes its scratch profile.
        l.install(APP, "xdg-data:ro;");
        assert_eq!(refused(pick(&l)), "launch workspace");
        // Read/write XDG data grant covers the root.
        l.install(APP, "xdg-data;");
        assert!(pick(&l).is_ok());
        // Explicit path, `home` and `~/` forms.
        for grant in [
            "~/.local/share/emuwiz;",
            "home;",
            &format!("{};", l.host.xdg_data_home.display()),
        ] {
            l.install(APP, grant);
            assert!(pick(&l).is_ok(), "{grant}");
        }
        // An override can revoke what the app metadata granted.
        l.install(APP, "host;");
        assert!(pick(&l).is_ok());
        l.user_override(APP, "!host;");
        assert_eq!(refused(pick(&l)), "launch workspace");
        // A global override can grant it again.
        fs::remove_file(l.host.xdg_data_home.join("flatpak/overrides").join(APP)).unwrap();
        l.user_override("global", "host;");
        assert!(pick(&l).is_ok());
        // Host never covers reserved paths such as /tmp.
        let grants = FlatpakFilesystems::from_layers(["[Context]\nfilesystems=host;\n"]);
        assert!(!grants.grants_read_write(&l.host, Path::new("/tmp/emuwiz/x")));
        assert!(!grants.grants_read_write(&l.host, Path::new("relative/path")));
    }

    #[test]
    fn every_resource_the_process_opens_is_checked_and_named() {
        let l = layout();
        let wrapper = l.flatpak_wrapper();
        let data = l.data_root();
        let mut n = Needs::new(&l, data.join("cheats-id"));
        // Only the workspace is granted; the app's own profile is always there.
        l.install(APP, "~/.local/share/emuwiz;");
        assert!(n.check(&wrapper, Some(&l.host)).is_ok());
        // Content on a mount the sandbox was never granted (and /tmp even with host).
        n.content = PathBuf::from("/mnt/roms/game.md");
        assert_eq!(refused(n.check(&wrapper, Some(&l.host))), "game content");
        l.install(APP, "~/.local/share/emuwiz;/mnt/roms:ro;");
        assert!(
            n.check(&wrapper, Some(&l.host)).is_ok(),
            "read-only content is enough"
        );
        l.install(APP, "host;");
        n.content = PathBuf::from("/tmp/fixture/game.md");
        assert_eq!(refused(n.check(&wrapper, Some(&l.host))), "game content");
        n.content = n.own.join("content/game.md");
        // Core outside the profile and ungranted.
        n.core = PathBuf::from("/opt/cores/core_libretro.so");
        assert!(n.check(&wrapper, Some(&l.host)).is_ok(), "host grants /opt");
        l.install(APP, "~/.local/share/emuwiz;");
        assert_eq!(refused(n.check(&wrapper, Some(&l.host))), "emulator core");
        n.core = n.own.join("cores/core_libretro.so");
        // Save and state directories must be writable, not just readable.
        n.saves = PathBuf::from("/mnt/saves");
        assert_eq!(refused(n.check(&wrapper, Some(&l.host))), "save directory");
        l.install(APP, "~/.local/share/emuwiz;/mnt/saves:ro;");
        assert_eq!(refused(n.check(&wrapper, Some(&l.host))), "save directory");
        l.install(APP, "~/.local/share/emuwiz;/mnt/saves;");
        assert!(n.check(&wrapper, Some(&l.host)).is_ok());
        n.states = PathBuf::from("/mnt/states");
        assert_eq!(
            refused(n.check(&wrapper, Some(&l.host))),
            "save-state directory"
        );
    }

    #[test]
    fn unknown_sandbox_and_missing_data_dir_fail_closed() {
        let l = layout();
        let bwrap = l.script("bw", "#!/bin/sh\nexec bwrap --dev-bind / / retroarch\n");
        assert_eq!(
            Needs::new(&l, l.data_root()).check(&bwrap, None),
            Err(RetroArchRootError::SandboxUnknown)
        );
        assert_eq!(
            select_retroarch_approved_root(&l.flatpak_wrapper(), None),
            Err(RetroArchRootError::DataDirUnavailable)
        );
    }
}
