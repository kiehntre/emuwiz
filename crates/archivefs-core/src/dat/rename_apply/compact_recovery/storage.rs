//! Linux compact evidence storage and exact runtime materialization. Writers
//! require the explicit default-OFF controller; ordinary application readers do not.
use super::*;
#[path = "integration.rs"]
pub(super) mod integration;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

const NAMESPACE: &str = "emuwiz-enrolled-recovery-v3";
const LEAF: &str = "container.v3";

// Never dirty a filesystem block containing another committed record.
const SLOT: u64 = 65536;
const COMMITTED: u8 = 0xa5;
const HEADER: usize = 64;
fn fail(text: impl std::fmt::Display) -> io::Error {
    io::Error::other(text.to_string())
}
fn protocol(e: Error) -> io::Error {
    fail(format!("invalid recovery evidence: {e:?}"))
}
fn add(a: u64, b: u64) -> io::Result<u64> {
    a.checked_add(b).ok_or_else(|| fail("size overflow"))
}
fn mul(a: u64, b: u64) -> io::Result<u64> {
    a.checked_mul(b).ok_or_else(|| fail("size overflow"))
}
fn round(n: u64) -> io::Result<u64> {
    mul(add(n, 4095)? / 4096, 4096)
}
fn identity(f: &File) -> io::Result<(u64, u64)> {
    let m = f.metadata()?;
    Ok((m.dev(), m.ino()))
}
fn at(f: &File, name: &str) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", f.as_raw_fd())).join(name)
}
fn open_dir(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(fail("absolute fixture path required"));
    }
    let mut fd = File::open("/")?;
    for c in path.components() {
        match c {
            Component::RootDir => {}
            Component::Normal(name) => {
                use std::os::unix::ffi::OsStrExt;
                let name = std::ffi::CString::new(name.as_bytes()).map_err(fail)?;
                let raw = unsafe {
                    libc::openat(
                        fd.as_raw_fd(),
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    )
                };
                if raw < 0 {
                    return Err(io::Error::last_os_error());
                }
                fd = unsafe { File::from_raw_fd(raw) };
            }
            _ => return Err(fail("non-normal directory path refused")),
        }
    }
    Ok(fd)
}
fn private(f: &File) -> io::Result<()> {
    let m = f.metadata()?;
    if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
        return Err(fail("private owner-only fixture required"));
    }
    Ok(())
}
fn lock(f: &File) -> io::Result<()> {
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(fail("fixture already owned by another writer"));
    }
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Ticket {
    root: PathBuf,
    root_id: (u64, u64),
    directory_id: (u64, u64),
    file_id: (u64, u64),
    binding: [u8; 32],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    Created,
    Allocated,
    Initialized,
    ReserveSync,
    DirectorySync,
    ParentSync,
    BaseWrite,
    BaseSync,
    BaseCommit,
    BaseCommitSync,
    RecordWrite,
    RecordSync,
    RecordCommit,
    RecordCommitSync,
}
trait Io {
    fn event(&mut self, _: Event) -> io::Result<()> {
        Ok(())
    }
    fn write(&mut self, f: &File, offset: u64, bytes: &[u8]) -> io::Result<()> {
        f.write_all_at(bytes, offset)
    }
}
struct Real;
impl Io for Real {}
struct Layout {
    slots: u64,
    base_end: u64,
    length: u64,
    charge: u64,
}
fn layout(base: &Authority, cap: u64) -> io::Result<Layout> {
    let slots = add(mul(base.transaction.entries.len() as u64, 12)?, 32)?;
    let base_end = round(add(
        mul(add(HEADER as u64, base.original.len() as u64)?, 2)?,
        1,
    )?)?;
    let length = add(base_end, mul(slots, SLOT)?)?;
    // Payload blocks plus explicit directory/metadata planning allowance.
    let charge = add(round(length)?, 65536)?;
    if charge > cap || length > i64::MAX as u64 || usize::try_from(length).is_err() {
        return Err(fail("storage budget exhausted"));
    }
    Ok(Layout {
        slots,
        base_end,
        length,
        charge,
    })
}
fn encoded(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .flat_map(|b| [(b >> 4) + 1, (b & 15) + 1])
        .collect()
}
fn decoded(bytes: &[u8]) -> io::Result<Vec<u8>> {
    if !bytes.len().is_multiple_of(2) || bytes.iter().any(|b| !(1..=16).contains(b)) {
        return Err(fail("corrupt encoded payload"));
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|p| ((p[0] - 1) << 4) | (p[1] - 1))
        .collect())
}
fn base_payload(base: &Authority, l: &Layout) -> Vec<u8> {
    let mut h = vec![0; HEADER];
    h[..8].copy_from_slice(b"EWSYNV3\0");
    h[8..16].copy_from_slice(&(base.original.len() as u64).to_le_bytes());
    h[16..24].copy_from_slice(&l.slots.to_le_bytes());
    h[24..56].copy_from_slice(&base.binding);
    h[56..64].copy_from_slice(&l.length.to_le_bytes());
    h.extend(&base.original);
    encoded(&h)
}
fn frame(base: &Authority, index: u64, fact: Fact, prev: [u8; 32]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend(MAGIC);
    v.extend(VERSION.to_le_bytes());
    v.extend(base.binding);
    v.extend(index.to_le_bytes());
    v.push(fact as u8);
    v.extend(prev);
    let h = Sha256::digest(&v);
    v.extend(h);
    v
}
struct Snapshot {
    stream: Vec<u8>,
    facts: BTreeMap<(u64, u8), Vec<u8>>,
    pending: Option<(u64, Vec<u8>)>,
    cursor: Vec<u8>,
    runtime: checkpoint::Runtime,
    count: u64,
}
struct Store {
    ticket: Ticket,
    root: File,
    directory: File,
    file: File,
    process: u32,
    layout: Layout,
}
impl Drop for Store {
    fn drop(&mut self) {
        if self.process == std::process::id() {
            unsafe {
                libc::flock(self.directory.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}
impl Store {
    fn reserve(root: &Path, base: &Authority, cap: u64, io: &mut impl Io) -> io::Result<Self> {
        let l = layout(base, cap)?;
        let parent = open_dir(root)?;
        private(&parent)?;
        // Exclusive mkdir: interrupted setup is preserved, never silently adopted.
        let name = std::ffi::CString::new(NAMESPACE).unwrap();
        if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(at(&parent, NAMESPACE))?;
        private(&directory)?;
        lock(&directory)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(at(&directory, LEAF))?;
        let ticket = Ticket {
            root: root.into(),
            root_id: identity(&parent)?,
            directory_id: identity(&directory)?,
            file_id: identity(&file)?,
            binding: base.binding,
        };
        let store = Self {
            ticket,
            root: parent,
            directory,
            file,
            process: std::process::id(),
            layout: l,
        };
        io.event(Event::Created)?;
        // Restrict this experimental reservation contract to local ext4.
        let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstatfs(store.file.as_raw_fd(), &mut stat) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if stat.f_type != libc::EXT4_SUPER_MAGIC || stat.f_bsize != 4096 {
            return Err(fail(
                "reservation experiment requires ext4 with 4096-byte blocks",
            ));
        }
        let result =
            unsafe { libc::posix_fallocate(store.file.as_raw_fd(), 0, store.layout.length as i64) };
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result));
        }
        io.event(Event::Allocated)?;
        // Convert reserved unwritten extents before admission, without sparse holes.
        let zeros = [0; 65536];
        let mut offset = 0;
        while offset < store.layout.length {
            let count = (store.layout.length - offset).min(zeros.len() as u64) as usize;
            io.write(&store.file, offset, &zeros[..count])?;
            offset += count as u64;
        }
        io.event(Event::Initialized)?;
        store.file.sync_all()?;
        io.event(Event::ReserveSync)?;
        store.directory.sync_all()?;
        io.event(Event::DirectorySync)?;
        store.root.sync_all()?;
        io.event(Event::ParentSync)?;
        store.bound()?;
        store.allocation()?;
        Ok(store)
    }
    fn reopen(ticket: &Ticket, base: &Authority, cap: u64) -> io::Result<Self> {
        if ticket.binding != base.binding {
            return Err(fail("foreign authority"));
        }
        let root = open_dir(&ticket.root)?;
        private(&root)?;
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(at(&root, NAMESPACE))?;
        private(&directory)?;
        lock(&directory)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(at(&directory, LEAF))?;
        let s = Self {
            ticket: ticket.clone(),
            root,
            directory,
            file,
            process: std::process::id(),
            layout: layout(base, cap)?,
        };
        s.bound()?;
        s.allocation()?;
        Ok(s)
    }
    fn inspect(ticket: &Ticket, base: &Authority, cap: u64) -> io::Result<Self> {
        if ticket.binding != base.binding {
            return Err(fail("foreign authority"));
        }
        let root = open_dir(&ticket.root)?;
        private(&root)?;
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(at(&root, NAMESPACE))?;
        private(&directory)?;
        if unsafe { libc::flock(directory.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } != 0 {
            return Err(fail("compact writer active; retry inspection"));
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(at(&directory, LEAF))?;
        let s = Self {
            ticket: ticket.clone(),
            root,
            directory,
            file,
            process: std::process::id(),
            layout: layout(base, cap)?,
        };
        s.bound()?;
        s.allocation()?;
        Ok(s)
    }
    fn bound(&self) -> io::Result<()> {
        if self.process != std::process::id() {
            return Err(fail("inherited writer authority refused"));
        }
        if identity(&open_dir(&self.ticket.root)?)? != self.ticket.root_id
            || identity(&self.root)? != self.ticket.root_id
        {
            return Err(fail("fixture root rebound"));
        }
        let d = std::fs::symlink_metadata(at(&self.root, NAMESPACE))?;
        let f = std::fs::symlink_metadata(at(&self.directory, LEAF))?;
        let held = self.file.metadata()?;
        if !d.is_dir()
            || (d.dev(), d.ino()) != self.ticket.directory_id
            || identity(&self.directory)? != self.ticket.directory_id
            || !f.is_file()
            || (f.dev(), f.ino()) != self.ticket.file_id
            || identity(&self.file)? != self.ticket.file_id
            || held.nlink() != 1
            || held.uid() != unsafe { libc::geteuid() }
            || held.mode() & 0o077 != 0
        {
            return Err(fail("storage binding or ownership changed"));
        }
        private(&self.root)?;
        private(&self.directory)?;
        Ok(())
    }
    fn allocation(&self) -> io::Result<u64> {
        let m = self.file.metadata()?;
        let blocks = mul(m.blocks(), 512)?;
        if m.len() != self.layout.length
            || blocks < round(m.len())?
            || add(blocks, mul(self.directory.metadata()?.blocks(), 512)?)? > self.layout.charge
        {
            return Err(fail("reservation size/blocks mismatch"));
        }
        Ok(blocks)
    }
    fn read(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut v = vec![0; len];
        self.file.read_exact_at(&mut v, offset)?;
        Ok(v)
    }
    fn publish_payload(
        &self,
        offset: u64,
        expected: &[u8],
        events: [Event; 4],
        io: &mut impl Io,
    ) -> io::Result<()> {
        self.bound()?;
        self.allocation()?;
        let held = self.read(offset, expected.len() + 1)?;
        let marker = held[expected.len()];
        if marker == COMMITTED {
            if held[..expected.len()] != *expected {
                return Err(fail("immutable committed payload conflicts"));
            }
            self.file.sync_all()?;
            return Ok(());
        }
        if marker != 0 {
            return Err(fail("corrupt commit marker"));
        }
        let prefix = held[..expected.len()]
            .iter()
            .position(|b| *b == 0)
            .unwrap_or(expected.len());
        if held[..prefix] != expected[..prefix] || held[prefix..].iter().any(|b| *b != 0) {
            return Err(fail("interrupted payload conflicts; evidence retained"));
        }
        io.write(&self.file, offset + prefix as u64, &expected[prefix..])?;
        io.event(events[0])?;
        self.file.sync_all()?;
        io.event(events[1])?;
        self.bound()?;
        io.write(&self.file, offset + expected.len() as u64, &[COMMITTED])?;
        io.event(events[2])?;
        self.file.sync_all()?;
        io.event(events[3])?;
        self.allocation()?;
        Ok(())
    }
    fn publish_base(&self, base: &Authority, io: &mut impl Io) -> io::Result<()> {
        if base.binding != self.ticket.binding {
            return Err(fail("foreign authority"));
        }
        self.publish_payload(
            0,
            &base_payload(base, &self.layout),
            [
                Event::BaseWrite,
                Event::BaseSync,
                Event::BaseCommit,
                Event::BaseCommitSync,
            ],
            io,
        )
    }
}
#[path = "checkpoint.rs"]
mod checkpoint;

const CAP: u64 = 64 * 1024 * 1024;
#[path = "dispatch.rs"]
mod dispatch;
use dispatch::publish_descriptor;
pub(crate) use dispatch::{discovery_bounded, discovery_path, is_enrolled, read, read_bounded};
