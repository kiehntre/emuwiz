//! Bounded /proc evidence. No environment files are read.
use super::*;
use safety::FileIdentity;
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};

fn identity(path: &Path) -> Option<FileIdentity> {
    let m = fs::metadata(path).ok()?;
    m.is_file().then_some(FileIdentity {
        device: m.dev(),
        inode: m.ino(),
    })
}
fn gone(base: &Path) -> bool {
    matches!(fs::symlink_metadata(base),Err(e) if e.kind()==std::io::ErrorKind::NotFound)
}
fn bounded(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    Ok(bytes)
}
// hidepid can suppress PID directories entirely: absence of an EACCES entry
// then says nothing about stopped state. Only the ordinary visible live /proc
// mount is accepted. Explicit synthetic roots remain the deterministic seam.
fn visible_mount_policy(bytes: &[u8]) -> bool {
    let mut found = false;
    for line in bytes.split(|b| *b == b'\n') {
        let fields: Vec<_> = line
            .split(|b| b.is_ascii_whitespace())
            .filter(|f| !f.is_empty())
            .collect();
        if fields.get(4) != Some(&b"/proc".as_slice()) {
            continue;
        }
        found = true;
        let Some(separator) = fields.iter().position(|f| *f == b"-") else {
            return false;
        };
        if fields.get(separator + 1) != Some(&b"proc".as_slice()) {
            return false;
        }
        if !fields
            .iter()
            .flat_map(|f| f.split(|b| *b == b','))
            .all(|option| {
                option
                    .strip_prefix(b"hidepid=")
                    .is_none_or(|value| value == b"0" || value == b"off")
            })
        {
            return false;
        }
    }
    found
}
fn visible_live_view(root: &Path) -> bool {
    root != Path::new("/proc")
        || bounded(&root.join("self/mountinfo"), 1024 * 1024)
            .is_ok_and(|bytes| visible_mount_policy(&bytes))
}
#[cfg(test)]
#[test]
fn hidden_or_unrecognised_live_proc_mounts_never_prove_stopped() {
    for option in ["rw", "rw,hidepid=0", "rw,hidepid=off"] {
        let text = format!("1 0 0:1 / /proc rw - proc proc {option}\n");
        assert!(visible_mount_policy(text.as_bytes()));
    }
    for option in [
        "rw,hidepid=1",
        "rw,hidepid=2",
        "rw,hidepid=4",
        "rw,hidepid=invisible",
        "rw,hidepid=unknown",
    ] {
        let text = format!("1 0 0:1 / /proc rw - proc proc {option}\n");
        assert!(!visible_mount_policy(text.as_bytes()));
    }
    assert!(!visible_mount_policy(
        b"1 0 0:1 / /proc rw - proc proc rw\n2 0 0:2 / /proc rw - proc proc rw,hidepid=2\n"
    ));
    assert!(!visible_mount_policy(b"malformed"));
    assert!(!visible_mount_policy(
        b"1 0 0:1 / /proc rw - tmpfs tmpfs rw\n"
    ));
}
// An existing PID without exe is not automatically vanished. Only a readable
// stat identifying a zombie or kernel thread proves it cannot execute this file.
fn no_userspace_executable(base: &Path) -> bool {
    let Ok(stat) = bounded(&base.join("stat"), 4096) else {
        return false;
    };
    let Some(close) = stat.iter().rposition(|b| *b == b')') else {
        return false;
    };
    let fields: Vec<_> = stat[close + 1..]
        .split(|b| b.is_ascii_whitespace())
        .filter(|s| !s.is_empty())
        .collect();
    fields.first().is_some_and(|s| *s == b"Z" || *s == b"X")
        || fields
            .get(6)
            .and_then(|v| std::str::from_utf8(v).ok())
            .and_then(|v| v.parse::<u64>().ok())
            .is_some_and(|flags| flags & 0x0020_0000 != 0)
}

pub(super) fn probe(
    executable: &Path,
    root: &Path,
    expected: Option<FileIdentity>,
) -> QuiescenceEvidence {
    let Some(expected) = expected else {
        return QuiescenceEvidence::Unknown;
    };
    if !visible_live_view(root) {
        return QuiescenceEvidence::Unknown;
    }
    let Ok(root_metadata) = fs::metadata(root) else {
        return QuiescenceEvidence::Unknown;
    };
    let root_identity = (root_metadata.dev(), root_metadata.ino());
    let Ok(entries) = fs::read_dir(root) else {
        return QuiescenceEvidence::Unknown;
    };
    let mut uncertain = false;
    let mut inspected = 0;
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => {
                uncertain = true;
                continue;
            }
        };
        let name = entry.file_name();
        let bytes = name.as_bytes();
        if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
            continue;
        }
        let base = entry.path();
        inspected += 1;
        let exe = match fs::read_link(base.join("exe")) {
            Ok(p) => p,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound
                    && (gone(&base) || no_userspace_executable(&base))
                {
                    continue;
                }
                uncertain = true;
                continue;
            }
        };
        let mut link = exe.as_os_str().as_bytes();
        if let Some(trimmed) = link.strip_suffix(b" (deleted)") {
            link = trimmed;
        }
        if link == executable.as_os_str().as_bytes()
            || identity(&base.join("exe")) == Some(expected)
        {
            return QuiescenceEvidence::Running;
        }
        if identity(&base.join("exe")).is_none() {
            if !gone(&base) {
                uncertain = true
            }
            continue;
        }
        // argv[0] is the only argument needed. Stop at its NUL terminator;
        // reject overlarge or unreadable evidence rather than reading argv/env.
        let first = File::open(base.join("cmdline")).and_then(|f| {
            use std::io::BufRead;
            let mut out = Vec::new();
            std::io::BufReader::new(f.take(64 * 1024 + 1)).read_until(0, &mut out)?;
            if out.len() > 64 * 1024 || out.pop() != Some(0) {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
            Ok(out)
        });
        match first {
            Ok(v) if v == executable.as_os_str().as_bytes() => return QuiescenceEvidence::Running,
            Ok(_) => {}
            Err(_) => {
                if !gone(&base) {
                    uncertain = true
                }
            }
        }
        // Mounted AppImages run a different inode and often a different argv0.
        // Without reading environments we cannot rule out that indirect use.
        if executable.as_os_str().as_bytes().ends_with(b".AppImage")
            && link.windows(8).any(|w| w == b"/.mount_")
        {
            uncertain = true
        }
    }
    if !fs::metadata(root).is_ok_and(|m| m.is_dir() && (m.dev(), m.ino()) == root_identity) {
        return QuiescenceEvidence::Unknown;
    }
    if !visible_live_view(root) {
        return QuiescenceEvidence::Unknown;
    }
    if inspected == 0 || uncertain {
        QuiescenceEvidence::Unknown
    } else {
        QuiescenceEvidence::Stopped
    }
}

pub(super) fn normal(executable: &Path, proc_root: &Path) -> QuiescenceEvidence {
    probe(executable, proc_root, identity(executable))
}

pub(super) fn recorded(journal: &UpdateJournal, proc_root: &Path) -> QuiescenceEvidence {
    let Ok(paths) = safety::Paths::journal(
        &match journal.record_path() {
            Ok(p) => p,
            Err(_) => return QuiescenceEvidence::Unknown,
        },
        journal,
    ) else {
        return QuiescenceEvidence::Unknown;
    };
    let mut images = Vec::new();
    match fs::symlink_metadata(&paths.target) {
        Ok(_) => {
            let Some(id) = identity(&paths.target) else {
                return QuiescenceEvidence::Unknown;
            };
            images.push(id);
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return QuiescenceEvidence::Unknown,
    }
    // Check every retained executable image, including an old image still held
    // by a process after publication. Missing install paths still match argv0.
    for (path, hash) in [
        (&paths.backup, &journal.original_sha256),
        (&paths.displaced, &journal.published_sha256),
    ] {
        match observe(path) {
            Ok(Observed::Missing) => {}
            Ok(Observed::Hash(h)) if h == *hash => {
                let Some(id) = identity(path) else {
                    return QuiescenceEvidence::Unknown;
                };
                images.push(id);
            }
            _ => return QuiescenceEvidence::Unknown,
        }
    }
    if images.is_empty() {
        return QuiescenceEvidence::Unknown;
    }
    let mut unknown = false;
    for image in images {
        match probe(&paths.target, proc_root, Some(image)) {
            QuiescenceEvidence::Running => return QuiescenceEvidence::Running,
            QuiescenceEvidence::Unknown => unknown = true,
            QuiescenceEvidence::Stopped => {}
        }
    }
    if unknown {
        QuiescenceEvidence::Unknown
    } else {
        QuiescenceEvidence::Stopped
    }
}
