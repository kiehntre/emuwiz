use super::*;
use ring::{
    hmac,
    rand::{SecureRandom, SystemRandom},
};
use std::{
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
};
const LIMIT: usize = 4096;
#[derive(Serialize, Deserialize)]
struct Sealed {
    payload: String,
    mac: String,
}
pub(super) struct Store {
    pub root: PathBuf,
    key: Vec<u8>,
    _directory: File,
}
impl Store {
    pub fn open(root: &Path, create: bool) -> Result<Self, String> {
        use std::os::fd::AsRawFd;
        let directory = open_directory(root, create)?;
        let m = directory.metadata().map_err(|e| e.to_string())?;
        if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
            return Err("Association metadata directory is not private".into());
        }
        // This internal path is pinned by the retained directory fd. It is
        // never accepted as a cartridge, save or imported evidence path.
        let root = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
        let path = root.join("capture.key");
        if create && !path.exists() {
            let mut key = [0; 32];
            SystemRandom::new()
                .fill(&mut key)
                .map_err(|_| "Capture key generation failed")?;
            let mut temp = tempfile::NamedTempFile::new_in(&root).map_err(|e| e.to_string())?;
            temp.write_all(&key)
                .and_then(|_| temp.as_file().sync_all())
                .map_err(|e| e.to_string())?;
            match temp.persist_noclobber(&path) {
                Ok(_) => {
                    directory.sync_all().map_err(|e| e.to_string())?;
                }
                Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        let (key_obj, mut file) = metadata_object(&path)?;
        if key_obj.stamp.mode & 0o077 != 0
            || key_obj.stamp.uid != unsafe { libc::geteuid() }
            || key_obj.stamp.size != 32
        {
            return Err("Capture key is unavailable or unsafe".into());
        }
        use std::io::Seek;
        file.rewind().map_err(|e| e.to_string())?;
        let mut key = Vec::new();
        file.read_to_end(&mut key).map_err(|e| e.to_string())?;
        Ok(Self {
            root: root.to_path_buf(),
            key,
            _directory: directory,
        })
    }
    pub fn revalidate_key(&self) -> Result<(), String> {
        let m = self._directory.metadata().map_err(|e| e.to_string())?;
        if m.mode() & 0o077 != 0 || m.uid() != unsafe { libc::geteuid() } {
            return Err("Capture metadata privacy changed".into());
        }
        let (_, mut file) = metadata_object(&self.root.join("capture.key"))?;
        use std::io::Seek;
        file.rewind().map_err(|e| e.to_string())?;
        let mut key = Vec::new();
        file.read_to_end(&mut key).map_err(|e| e.to_string())?;
        if key != self.key {
            return Err("Capture seal key changed during session".into());
        }
        Ok(())
    }
    pub fn publish(&self, name: &str, receipt: &Receipt) -> Result<(), String> {
        let payload = serde_json::to_string(receipt).map_err(|e| e.to_string())?;
        let tag = hmac::sign(
            &hmac::Key::new(hmac::HMAC_SHA256, &self.key),
            payload.as_bytes(),
        );
        let value = serde_json::to_vec(&Sealed {
            payload,
            mac: hex(tag.as_ref()),
        })
        .map_err(|e| e.to_string())?;
        let mut tmp = tempfile::NamedTempFile::new_in(&self.root).map_err(|e| e.to_string())?;
        tmp.write_all(&value)
            .and_then(|_| tmp.as_file().sync_all())
            .map_err(|e| e.to_string())?;
        tmp.persist_noclobber(self.root.join(name))
            .map_err(|e| e.to_string())?;
        File::open(&self.root)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn receipts(&self) -> Result<Vec<Receipt>, String> {
        let mut receipts = Vec::new();
        let mut count = 0;
        for e in fs::read_dir(&self.root).map_err(|e| e.to_string())? {
            let path = e.map_err(|e| e.to_string())?.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            count += 1;
            if count > LIMIT {
                return Err("Association metadata limit reached; no guesses were made".into());
            }
            let (obj, mut f) = metadata_object(&path)?;
            if obj.stamp.size > 256 * 1024 || obj.stamp.mode & 0o077 != 0 {
                return Err("Unsafe capture receipt".into());
            }
            use std::io::Seek;
            f.rewind().map_err(|e| e.to_string())?;
            let signed: Sealed =
                serde_json::from_reader(f).map_err(|_| "Malformed association evidence")?;
            let mac = unhex(&signed.mac)?;
            hmac::verify(
                &hmac::Key::new(hmac::HMAC_SHA256, &self.key),
                signed.payload.as_bytes(),
                &mac,
            )
            .map_err(|_| "Association signature mismatch")?;
            let r: Receipt =
                serde_json::from_str(&signed.payload).map_err(|_| "Invalid association payload")?;
            if r.version != 1 {
                return Err("Unsupported association version".into());
            }
            receipts.push(r);
        }
        Ok(receipts)
    }
}
fn metadata_object(path: &Path) -> Result<(Object, File), String> {
    let f = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| e.to_string())?;
    if !f.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Metadata must be a regular file".into());
    }
    if f.metadata().map_err(|e| e.to_string())?.len() > 256 * 1024 {
        return Err("Capture metadata exceeds bounds".into());
    }
    object_file(path, f)
}
fn hex(b: &[u8]) -> String {
    b.iter().map(|v| format!("{v:02x}")).collect()
}
fn unhex(v: &str) -> Result<Vec<u8>, String> {
    if v.len() != 64 {
        return Err("Invalid association seal".into());
    }
    v.as_bytes()
        .chunks_exact(2)
        .map(|c| {
            std::str::from_utf8(c)
                .ok()
                .and_then(|s| u8::from_str_radix(s, 16).ok())
                .ok_or_else(|| "Invalid association seal".into())
        })
        .collect()
}
pub(super) fn session_id() -> Result<String, String> {
    let mut b = [0; 16];
    SystemRandom::new()
        .fill(&mut b)
        .map_err(|_| "Session ID generation failed")?;
    Ok(hex(&b))
}
