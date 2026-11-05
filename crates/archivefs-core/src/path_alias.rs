//! One installed program can be reachable through several spellings: on a
//! merged-/usr system `/bin` is a symlink to `/usr/bin`, so a `PATH` that lists
//! both makes the same `hatari` look like two different executables. Discovery
//! that counts candidates (and refuses "more than one") must count programs, not
//! spellings.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Removes entries that name the same file through different directory
/// spellings. Entries are compared by their canonical path; among entries that
/// share one, the entry whose path already *is* the canonical path is kept
/// (otherwise the first). Entries whose path cannot be canonicalised are always
/// kept, as is the relative order of everything that survives.
///
/// This only collapses aliases of one file. Two distinct installations (for
/// example `/usr/bin/x` and `/opt/x/x`) have different canonical paths and both
/// stay, so genuine ambiguity is still reported.
pub fn dedup_by_real_path<T>(items: &mut Vec<T>, path_of: impl Fn(&T) -> &Path) {
    let real: Vec<Option<PathBuf>> = items
        .iter()
        .map(|item| std::fs::canonicalize(path_of(item)).ok())
        .collect();
    let mut keeper: HashMap<&Path, usize> = HashMap::new();
    for (index, item) in items.iter().enumerate() {
        let Some(canonical) = real[index].as_deref() else {
            continue;
        };
        match keeper.get(canonical) {
            None => {
                keeper.insert(canonical, index);
            }
            Some(&current) => {
                let current_is_canonical = path_of(&items[current]) == canonical;
                if !current_is_canonical && path_of(item) == canonical {
                    keeper.insert(canonical, index);
                }
            }
        }
    }
    let mut index = 0;
    items.retain(|_| {
        let keep = match real[index].as_deref() {
            Some(canonical) => keeper.get(canonical) == Some(&index),
            None => true,
        };
        index += 1;
        keep
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn directory_alias_collapses_to_the_canonical_spelling() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("usr_bin");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("tool"), b"x").unwrap();
        let alias = dir.path().join("bin");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let canonical = std::fs::canonicalize(real.join("tool")).unwrap();

        // The alias sorts first, as /bin/hatari sorts before /usr/bin/hatari.
        let mut paths = vec![alias.join("tool"), canonical.clone()];
        dedup_by_real_path(&mut paths, |p| p.as_path());
        assert_eq!(paths, vec![canonical]);
    }

    #[test]
    fn distinct_installations_both_stay() {
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("a");
        let two = dir.path().join("b");
        std::fs::write(&one, b"1").unwrap();
        std::fs::write(&two, b"2").unwrap();
        let mut paths = vec![one.clone(), two.clone()];
        dedup_by_real_path(&mut paths, |p| p.as_path());
        assert_eq!(paths, vec![one, two]);
    }

    #[test]
    fn missing_paths_are_kept_untouched() {
        let mut paths = vec![
            PathBuf::from("/nonexistent/a"),
            PathBuf::from("/nonexistent/b"),
        ];
        dedup_by_real_path(&mut paths, |p| p.as_path());
        assert_eq!(paths.len(), 2);
    }
}
