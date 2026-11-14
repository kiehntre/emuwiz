//! Who may run the relocation executor. Fail closed.
//!
//! Every mutating entry point (`apply`, `recover`, `recover_all`,
//! `apply_undo`) requires a `ScratchAuthority` in its [`Context`]. In this
//! release that is the **only** way to execute a relocation, and it exists
//! only inside this crate's own unit-test binary:
//!
//! - the type is crate-private and every way to make one is `#[cfg(test)]`.
//!   That is not a Cargo feature: it is set only when rustc compiles this
//!   crate's unit tests, never for a dependency. So no build of `emuwiz-cli`
//!   or Blue can contain or reach it, whatever the profile (debug, release)
//!   and whatever the features (including `--all-features`), and neither can
//!   their test binaries, this crate's integration tests or its doctests.
//!   `Context::with_authority` is crate-private and `#[cfg(test)]` too, and
//!   the context's authority field is crate-private. Outside the crate,
//!   `Context::new` is the only constructor and always carries none, so
//!   `apply` refuses with [`PRODUCTION_UNAVAILABLE`] before it reads the
//!   plan, and recovery refuses before it locks or writes anything. Nothing
//!   in a game folder, a configuration file or an environment variable
//!   can create one;
//! - it authorises only library roots **it created itself**: fresh,
//!   private (0700) directories with a random suffix, owned by this user,
//!   at canonical paths. It never adopts a directory named by the
//!   environment, a file or a plan. The single exception is test-internal:
//!   a parent test adopts the roots its own child process just created, read
//!   from that child's stdout pipe, and only inside a folder the parent made
//!   for it. The child's environment names only parent folders, and the
//!   child builds a fresh fixture inside them. Their
//!   (device, inode, owner) identity is held in the authority (in memory,
//!   outside every candidate root) and re-checked before every step, so a
//!   replaced directory, a symlink, a bind mount over the path or a remount
//!   on another device is refused;
//! - it selects [`FsPolicy::TestScratch`]. Every drive durability rule
//!   still applies under it: ext4 only, journal in use, barriers on,
//!   `data=ordered|journal`, not read-only, no nested or bind mount, the
//!   same mount facts as planned, and the same directory-fsync ordering. The
//!   one difference is that ext4's error behaviour may be `errors=continue`
//!   (this host's test filesystems). Results obtained that way are
//!   process-death and fault-injection evidence. They are **not**
//!   power-loss evidence, and the production policy is not relaxed by them.
//!
//! What callers outside the crate can and cannot write (doctests compile as
//! an external crate, so these hold with any feature set):
//!
//! ```
//! // The ordinary context compiles, and it can execute nothing.
//! let ctx = archivefs_core::transform::Context::new(std::env::temp_dir().join("emuwiz-no-such-journal"));
//! let _ = archivefs_core::transform::PRODUCTION_UNAVAILABLE;
//! # let _ = ctx;
//! ```
//!
//! ```compile_fail,E0432
//! use archivefs_core::transform::ScratchAuthority; // not exported
//! ```
//!
//! ```compile_fail,E0603
//! use archivefs_core::transform::authority::ScratchAuthority; // crate-private
//! ```
//!
//! ```compile_fail,E0599
//! // No way to attach an authority to a context.
//! let ctx = archivefs_core::transform::Context::new("/nonexistent");
//! let _ = ctx.with_authority(unreachable!());
//! ```
//!
//! ```compile_fail,E0616
//! // The field is crate-private.
//! let ctx = archivefs_core::transform::Context::new("/nonexistent");
//! let _ = ctx.authority;
//! ```
//!
//! ```compile_fail,E0432
//! // The scratch planner does not exist outside the crate's unit tests.
//! use archivefs_core::transform::plan::plan_relocation_scratch;
//! ```
//!
//! A future production move capability needs its own authorisation model,
//! an independent Tier-A review and separate deployment approval; nothing
//! here is a switch for it.
//!
//! [`Context`]: super::engine::Context
//! [`FsPolicy::TestScratch`]: super::fscompat::FsPolicy::TestScratch

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Why execution is refused outside the test environment.
pub const PRODUCTION_UNAVAILABLE: &str = "relocation cannot run in this build: the executor is available only \
inside EmuWiz's test suite on scratch folders it created itself. Production relocation needs a separate \
authorisation model, an independent safety review and the owner's approval. Nothing was changed.";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScratchRoot {
    path: PathBuf,
    dev: u64,
    ino: u64,
    uid: u32,
}

/// Test-only authority to execute relocations on scratch roots it created.
/// Crate-private; constructible only under `cfg(test)`.
#[derive(Debug)]
pub(crate) struct ScratchAuthority {
    roots: Vec<ScratchRoot>,
    policy: super::fscompat::FsPolicy,
}

impl ScratchAuthority {
    /// An authority with no roots yet.
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self {
            roots: Vec::new(),
            policy: super::fscompat::FsPolicy::TestScratch,
        }
    }

    /// An authority whose plans must meet the full production filesystem
    /// policy (`errors=remount-ro` included), for disposable strict ext4
    /// fixtures. It executes only its own plans, never a Production plan.
    #[cfg(test)]
    pub(crate) fn strict() -> Self {
        Self {
            roots: Vec::new(),
            policy: super::fscompat::FsPolicy::TestStrict,
        }
    }

    /// The filesystem policy this authority plans and executes under.
    pub(crate) fn policy(&self) -> super::fscompat::FsPolicy {
        self.policy
    }

    /// Creates a fresh private scratch library root inside `parent` (which
    /// must exist) and authorises exactly that directory.
    #[cfg(test)]
    pub(crate) fn provision_root(&mut self, parent: &Path, name: &str) -> std::io::Result<PathBuf> {
        use std::os::unix::fs::DirBuilderExt;
        let parent = std::fs::canonicalize(parent)?;
        let id = super::journal::new_txn_id()?;
        let path = parent.join(format!("{name}-{}", &id[..12]));
        // Not recursive: an existing directory is refused, never adopted.
        std::fs::DirBuilder::new().mode(0o700).create(&path)?;
        std::fs::File::open(&parent)?.sync_all()?;
        let m = std::fs::symlink_metadata(&path)?;
        self.roots.push(ScratchRoot {
            path: path.clone(),
            dev: m.dev(),
            ino: m.ino(),
            uid: m.uid(),
        });
        Ok(path)
    }

    /// Parent side of a child-process test: adopts a scratch root that a
    /// child process this test spawned created, read from that child's
    /// stdout pipe (never from the environment or a file). It must be a real
    /// directory owned by this user, at its canonical path, exactly two
    /// levels below `nursery`, a folder this test created for the child.
    #[cfg(test)]
    pub(crate) fn adopt_from_child(&mut self, nursery: &Path, root: &Path) -> Result<(), String> {
        let nursery = std::fs::canonicalize(nursery).map_err(|e| e.to_string())?;
        let real = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
        if real != root || real.parent().and_then(Path::parent) != Some(nursery.as_path()) {
            return Err(format!(
                "{} is not a root the child created inside {}",
                root.display(),
                nursery.display()
            ));
        }
        let m = std::fs::symlink_metadata(root).map_err(|e| e.to_string())?;
        let uid = unsafe { libc::geteuid() };
        if !m.is_dir() || m.uid() != uid {
            return Err(format!(
                "{} is not a directory owned by this user",
                root.display()
            ));
        }
        self.roots.push(ScratchRoot {
            path: real.clone(),
            dev: m.dev(),
            ino: m.ino(),
            uid,
        });
        self.admits(&real)
    }

    /// Whether `root` is one of this authority's roots, still the very
    /// directory it created.
    pub(crate) fn admits(&self, root: &Path) -> Result<(), String> {
        let deny = |why: &str| {
            format!(
                "{} is not an authorised scratch root ({why}); nothing was changed",
                root.display()
            )
        };
        let rec = self
            .roots
            .iter()
            .find(|r| r.path == root)
            .ok_or_else(|| deny("not created by this test"))?;
        let m = std::fs::symlink_metadata(root).map_err(|e| deny(&e.to_string()))?;
        if m.file_type().is_symlink() || !m.is_dir() {
            return Err(deny("it is no longer a real directory"));
        }
        if (m.dev(), m.ino(), m.uid()) != (rec.dev, rec.ino, rec.uid) {
            return Err(deny(
                "it was replaced, remounted or substituted since it was created",
            ));
        }
        match std::fs::canonicalize(root) {
            Ok(real) if real == rec.path => Ok(()),
            _ => Err(deny("its path no longer resolves to itself")),
        }
    }
}
