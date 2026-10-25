use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    os::unix::ffi::OsStringExt,
    os::unix::net::UnixStream,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32},
    },
    time::Duration,
};
#[derive(Clone, Debug)]
struct Pending {
    path: PathBuf,
    stamp: Stamp,
    bytes: Option<Vec<u8>>,
}
#[derive(Default)]
pub(super) struct Transcript {
    initialized: bool,
    mapped: bool,
    configured: bool,
    loads: usize,
    loaded: bool,
    active: bool,
    ended: bool,
    pub reason: Option<String>,
    open: BTreeMap<(u32, u64), Pending>,
    read_paths: BTreeSet<PathBuf>,
    read_verified: BTreeSet<PathBuf>,
    pub artifacts: BTreeMap<PathBuf, Object>,
}
impl Transcript {
    fn refuse(&mut self, why: impl Into<String>) {
        if self.reason.is_none() {
            self.reason = Some(why.into());
        }
    }
    pub fn complete(&self) -> bool {
        self.initialized
            && self.mapped
            && self.configured
            && self.loads == 1
            && self.loaded
            && self.ended
            && !self.active
            && self.open.is_empty()
            && self.reason.is_none()
    }
    fn file(
        &mut self,
        kind: u32,
        payload: &[u8],
        ctx: &Context,
        eligible: &BTreeMap<PathBuf, SramOrigin>,
    ) -> Result<(), String> {
        let mut r = Wire(payload);
        let fd = r.u32()?;
        let flags = r.u32()?;
        let token = r.u64()?;
        let stamp = r.stamp()?;
        let n = r.u32()? as usize;
        if fd < 3 || n > 4095 {
            return Err("Invalid observer descriptor/path".into());
        }
        let path = PathBuf::from(std::ffi::OsString::from_vec(r.bytes(n)?.to_vec()));
        let n = r.u32()? as usize;
        if n > MAX_RAM {
            return Err("Observer SRAM limit exceeded".into());
        }
        let bytes = r.bytes(n)?.to_vec();
        if !r.0.is_empty() {
            return Err("Trailing observer data".into());
        }
        if !path.starts_with(&ctx.scope.saves) || !path.is_absolute() {
            return Err("SRAM accessed outside the selected save scope".into());
        }
        let key = (fd, token);
        match kind {
            4 => {
                if self.open.keys().any(|(other, _)| *other == fd) {
                    return Err("Ambiguous descriptor reuse".into());
                }
                if flags & 1 != 0 {
                    self.read_paths.insert(path.clone());
                    if eligible
                        .get(&path)
                        .is_none_or(|e| e.artifact.stamp != stamp)
                    {
                        return Err(
                            "An unattested or changed SRAM artifact was opened for reading".into(),
                        );
                    }
                }
                self.open.insert(
                    key,
                    Pending {
                        path,
                        stamp,
                        bytes: None,
                    },
                );
            }
            5 => {
                let p = self
                    .open
                    .get(&key)
                    .ok_or("Read without an observed descriptor open")?;
                if p.path != path || !p.stamp.object(&stamp) {
                    return Err("Read descriptor identity changed".into());
                }
                if let Some(e) = eligible.get(&path) {
                    if e.artifact.stamp == stamp
                        && bytes.len() as u64 == stamp.size
                        && sha(&bytes) == e.artifact.sha256
                    {
                        self.read_verified.insert(path);
                    } else {
                        return Err("Historical SRAM read did not match its sealed evidence".into());
                    }
                } else {
                    return Err("Unattested historical SRAM read".into());
                }
            }
            8 => {
                if !self.active || self.loads != 1 {
                    return Err("SRAM write without one verified loaded cartridge".into());
                }
                let p = self
                    .open
                    .get_mut(&key)
                    .ok_or("Write without an observed descriptor open")?;
                if p.path != path || !p.stamp.object(&stamp) {
                    return Err("Write descriptor identity changed".into());
                }
                self.artifacts.remove(&path);
                p.bytes = if flags == 2 && !bytes.is_empty() {
                    Some(bytes)
                } else {
                    None
                };
            }
            10 => {
                let p = self
                    .open
                    .remove(&key)
                    .ok_or("Close without an observed descriptor lifetime")?;
                if p.path != path || !p.stamp.object(&stamp) {
                    return Err("Close descriptor identity changed".into());
                }
                if let Some(bytes) = p.bytes {
                    if stamp.links != 1
                        || stamp.size != bytes.len() as u64
                        || stamp.mode & libc::S_IFMT != libc::S_IFREG
                    {
                        return Err("Ambiguous SRAM artifact at close".into());
                    }
                    if self.read_paths.contains(&path) && !self.read_verified.contains(&path) {
                        return Err("Unattested or changed historical SRAM was opened/read".into());
                    }
                    self.artifacts.insert(
                        path.clone(),
                        Object {
                            path,
                            stamp,
                            sha256: sha(&bytes),
                        },
                    );
                }
            }
            _ => return Err("Unknown file event".into()),
        }
        Ok(())
    }
}
struct Wire<'a>(&'a [u8]);
impl<'a> Wire<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        if n > self.0.len() {
            return Err("Truncated observer event".into());
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }
    fn stamp(&mut self) -> Result<Stamp, String> {
        let mut v = [0; 10];
        for n in &mut v {
            *n = self.u64()?;
        }
        Ok(Stamp {
            device: v[0],
            inode: v[1],
            size: v[2],
            modified: v[3] as i64,
            modified_ns: v[4] as i64,
            changed: v[5] as i64,
            changed_ns: v[6] as i64,
            links: v[7],
            mode: v[8] as u32,
            uid: v[9] as u32,
        })
    }
}
pub(super) fn mapping_matches(maps: &str, address: u64, expected: &Stamp) -> bool {
    maps.lines().any(|line| {
        let p: Vec<_> = line.split_whitespace().collect();
        if p.len() < 5 || !p[1].contains('x') {
            return false;
        }
        let Some((a, b)) = p[0].split_once('-') else {
            return false;
        };
        let Some((major, minor)) = p[3].split_once(':') else {
            return false;
        };
        let (Ok(a), Ok(b), Ok(major), Ok(minor), Ok(ino)) = (
            u64::from_str_radix(a, 16),
            u64::from_str_radix(b, 16),
            u64::from_str_radix(major, 16),
            u64::from_str_radix(minor, 16),
            p[4].parse::<u64>(),
        ) else {
            return false;
        };
        address >= a
            && address < b
            && ino == expected.inode
            && major == libc::major(expected.device) as u64
            && minor == libc::minor(expected.device) as u64
    })
}
pub(super) fn read(
    mut stream: UnixStream,
    ctx: Context,
    eligible: BTreeMap<PathBuf, SramOrigin>,
    pid: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
) -> Transcript {
    let mut t = Transcript::default();
    let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
    let result = (|| -> Result<(), String> {
        let mut total = 0;
        loop {
            let mut head = [0; 16];
            read_exact(&mut stream, &mut head, &stop)?;
            let mut r = Wire(&head);
            let kind = r.u32()?;
            let n = r.u32()? as usize;
            let sender = r.u32()?;
            let reserved = r.u32()?;
            total += n + 16;
            if n > 8 * 1024 * 1024 || total > 32 * 1024 * 1024 || reserved != 0 {
                return Err("Observer bounds/protocol exceeded".into());
            }
            while pid.load(Ordering::Acquire) == 0 && !stop.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(1));
            }
            if sender != pid.load(Ordering::Acquire) {
                return Err("Unexpected observer process".into());
            }
            let mut bytes = vec![0; n];
            read_exact(&mut stream, &mut bytes, &stop)?;
            match kind {
                1 if !t.initialized && n == 0 => t.initialized = true,
                2 if t.initialized && !t.mapped && n == 8 => {
                    let address = u64::from_le_bytes(bytes.as_slice().try_into().unwrap());
                    let actual =
                        fs::metadata(format!("/proc/{sender}/exe")).map_err(|e| e.to_string())?;
                    let maps = fs::read_to_string(format!("/proc/{sender}/maps"))
                        .map_err(|e| e.to_string())?;
                    t.mapped = Stamp::from_metadata(&actual).object(&ctx.executable.stamp)
                        && mapping_matches(&maps, address, &ctx.core.stamp);
                    stream
                        .write_all(&[u8::from(t.mapped)])
                        .map_err(|e| e.to_string())?;
                    if !t.mapped {
                        return Err("Loaded executable/core mapping differs from retained preflight objects".into());
                    }
                }
                11 if t.initialized && n == 80 => {
                    let mut wire = Wire(&bytes);
                    if wire.stamp()? != ctx.scope.configuration.stamp {
                        return Err(
                            "Opened native configuration differs from selected scope".into()
                        );
                    }
                    t.configured = true;
                }
                3 if t.mapped => {
                    t.loads += 1;
                    let valid = t.loads == 1 && sha(&bytes) == ctx.cartridge.sha256;
                    stream
                        .write_all(&[u8::from(valid)])
                        .map_err(|e| e.to_string())?;
                    if !valid {
                        return Err("Additional or different cartridge load".into());
                    }
                }
                6 if n == 4 && t.loads == 1 && !t.loaded => {
                    if bytes != 1u32.to_le_bytes() {
                        return Err("Cartridge load failed".into());
                    }
                    t.loaded = true;
                    t.active = true;
                }
                7 if n == 0 && t.active => {
                    t.active = false;
                }
                4 | 5 | 8 | 10 => t.file(kind, &bytes, &ctx, &eligible)?,
                9 if n == 4 => {
                    if bytes != 0u32.to_le_bytes() {
                        return Err(
                            "Unsupported state load, fork, partial I/O or observer failure".into(),
                        );
                    }
                    t.ended = true;
                    break;
                }
                _ => return Err("Unexpected observer lifecycle event".into()),
            }
        }
        if !t.complete() {
            return Err("Incomplete observed cartridge lifecycle".into());
        }
        Ok(())
    })();
    if let Err(e) = result {
        t.refuse(e);
    }
    t
}
fn read_exact(s: &mut UnixStream, mut b: &mut [u8], stop: &AtomicBool) -> Result<(), String> {
    while !b.is_empty() {
        if stop.load(Ordering::Acquire) {
            return Err("Interrupted observer stream".into());
        }
        match s.read(b) {
            Ok(0) => return Err("Incomplete observer stream".into()),
            Ok(n) => b = &mut b[n..],
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}
