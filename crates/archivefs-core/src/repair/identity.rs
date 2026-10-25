//! Deterministic repair labels. These never replace filesystem/content proof.
use sha2::{Digest, Sha256};

/// Frame every field so different field boundaries cannot alias. Callers
/// include a domain/version and exact bytes rather than lossy display paths.
pub(super) fn digest_parts(parts: &[&[u8]]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part);
    }
    use std::fmt::Write;
    let mut hex = String::with_capacity(64);
    for byte in digest.finalize() {
        write!(&mut hex, "{byte:02x}").expect("writing to a String is infallible");
    }
    hex
}

pub(super) fn content_path_digest(content: &str, path: &std::path::Path) -> String {
    digest_parts(&[
        b"repair-content-path-v1",
        content.as_bytes(),
        path.as_os_str().as_encoded_bytes(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn job10_digest_fields_are_framed() {
        assert_ne!(digest_parts(&[b"ab", b"c"]), digest_parts(&[b"a", b"bc"]));
    }
    #[cfg(unix)]
    #[test]
    fn job10_digest_uses_exact_path_bytes_when_lossy_displays_match() {
        use std::os::unix::ffi::OsStrExt;
        let a = std::path::Path::new(std::ffi::OsStr::from_bytes(b"roms/\xff/track.bin"));
        let b = std::path::Path::new(std::ffi::OsStr::from_bytes(b"roms/\xfe/track.bin"));
        assert_eq!(a.to_string_lossy(), b.to_string_lossy());
        assert_ne!(
            content_path_digest("content", a),
            content_path_digest("content", b)
        );
        assert_eq!(
            content_path_digest("content", a),
            content_path_digest("content", a)
        );
    }
}
