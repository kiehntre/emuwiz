//! Reserved recovery namespaces are not game inputs. This lexical exclusion is
//! scan hygiene only: it grants neither ownership nor deletion authority. It
//! also covers explicitly selected descendants when authoritative records are
//! missing, so retained data is never silently reclassified as ordinary games.
use std::path::Path;
pub(crate) fn is_private_recovery_path(path: &Path) -> bool {
    path.ancestors().any(|part| {
        part.file_name().is_some_and(|name| {
            name == ".emuwiz-r2-holding"
                || name == "emuwiz-enrolled-recovery-v3"
                || name.to_string_lossy().starts_with(".emuwiz-copy-")
        })
    })
}

/// Resolve a selected root once so an explicit symlink/ancestor alias cannot
/// expose private recovery data. This is discovery hygiene at inspection time,
/// not proof of stable pathname ownership or deletion authority.
pub(crate) fn is_private_recovery_root(path: &Path) -> bool {
    is_private_recovery_path(path)
        || std::fs::canonicalize(path).is_ok_and(|resolved| is_private_recovery_path(&resolved))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn r2_private_recovery_entries_exclude_resolved_roots_and_ancestor_aliases() {
        let home = tempfile::tempdir().unwrap();
        for name in [
            ".emuwiz-r2-holding",
            "emuwiz-enrolled-recovery-v3",
            ".emuwiz-copy-fixture",
        ] {
            let private = home.path().join(name);
            std::fs::create_dir_all(private.join("tx")).unwrap();
            let alias = home.path().join(format!("alias-{name}"));
            std::os::unix::fs::symlink(&private, &alias).unwrap();
            assert!(is_private_recovery_root(&alias));
            assert!(is_private_recovery_root(&alias.join("tx")));
            assert!(!is_private_recovery_path(&alias));
        }
        assert!(!is_private_recovery_root(home.path()));
    }
}
