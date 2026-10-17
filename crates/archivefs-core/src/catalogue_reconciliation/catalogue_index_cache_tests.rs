use super::*;
use crate::dat::parsers::parse_dat_file;
use std::fs;

fn dat_file(dir: &Path, name: &str, roms: usize) -> std::path::PathBuf {
    let mut xml = format!(
        "<?xml version=\"1.0\"?>\n<datafile><header><name>{name}</name><description>{name}</description></header>\n"
    );
    for i in 0..roms {
        xml.push_str(&format!(
            "<game name=\"Game {i}\"><rom name=\"g{i}.bin\" size=\"4\" sha1=\"{:040x}\"/></game>\n",
            i + 1
        ));
    }
    xml.push_str("</datafile>\n");
    let path = dir.join(format!("{name}.dat"));
    fs::write(&path, xml).unwrap();
    path
}

fn built(path: &Path) -> Arc<CachedCatalogue> {
    let parsed = parse_dat_file(path, Default::default()).unwrap().dat;
    Arc::new(CachedCatalogue::build(
        path.display().to_string(),
        "test".into(),
        &parsed,
    ))
}

fn fingerprint(path: &Path) -> CatalogueFingerprint {
    CatalogueFingerprint::of_file(path, None, None).unwrap()
}

#[test]
fn a_repeated_lookup_reuses_the_same_index() {
    let dir = tempfile::tempdir().unwrap();
    let path = dat_file(dir.path(), "A", 10);
    let mut cache = CatalogueIndexCache::default();
    let key = "local:A".to_string();
    assert!(cache.lookup(&key, &fingerprint(&path)).is_none());
    let catalogue = built(&path);
    assert!(cache.insert(key.clone(), fingerprint(&path), catalogue.clone()));
    let again = cache.lookup(&key, &fingerprint(&path)).unwrap();
    assert!(Arc::ptr_eq(&again, &catalogue));
    assert_eq!(cache.stats().hits, 1);
    assert_eq!(catalogue.total_roms, 10);
    assert_eq!(catalogue.comparable_roms, 10);
}

#[test]
fn a_changed_or_replaced_dat_is_a_miss_and_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dat_file(dir.path(), "A", 3);
    let mut cache = CatalogueIndexCache::default();
    cache.insert("A".into(), fingerprint(&path), built(&path));
    // Rewritten in place with a different size.
    dat_file(dir.path(), "A", 4);
    assert!(cache.lookup("A", &fingerprint(&path)).is_none());
    assert_eq!(cache.stats().entries, 0, "the stale entry is removed");
    // Replaced by a new file (new inode).
    cache.insert("A".into(), fingerprint(&path), built(&path));
    let replacement = dir.path().join("new.dat");
    fs::copy(&path, &replacement).unwrap();
    fs::rename(&replacement, &path).unwrap();
    assert!(cache.lookup("A", &fingerprint(&path)).is_none());
    // A different recorded content SHA-256 or revision is also a miss.
    let base = fingerprint(&path);
    cache.insert("A".into(), base.clone(), built(&path));
    let other = CatalogueFingerprint {
        content_sha256: Some("ab".repeat(32)),
        ..base.clone()
    };
    assert!(cache.lookup("A", &other).is_none());
    cache.insert("A".into(), base.clone(), built(&path));
    let other = CatalogueFingerprint {
        revision: Some("2026-10-17".into()),
        ..base
    };
    assert!(cache.lookup("A", &other).is_none());
}

#[test]
fn disabled_or_removed_catalogues_are_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let a = dat_file(dir.path(), "A", 2);
    let b = dat_file(dir.path(), "B", 2);
    let mut cache = CatalogueIndexCache::default();
    cache.insert("A".into(), fingerprint(&a), built(&a));
    cache.insert("B".into(), fingerprint(&b), built(&b));
    cache.retain_only(&BTreeSet::from(["B".to_string()]));
    assert!(cache.lookup("A", &fingerprint(&a)).is_none());
    assert!(cache.lookup("B", &fingerprint(&b)).is_some());
}

#[test]
fn the_byte_budget_evicts_least_recently_used_and_refuses_oversized() {
    let dir = tempfile::tempdir().unwrap();
    let a = dat_file(dir.path(), "A", 100);
    let b = dat_file(dir.path(), "B", 100);
    let c = dat_file(dir.path(), "C", 100);
    let one = built(&a).estimated_bytes;
    // Room for two catalogues, not three.
    let mut cache = CatalogueIndexCache::with_budget(one * 2 + one / 2);
    cache.insert("A".into(), fingerprint(&a), built(&a));
    cache.insert("B".into(), fingerprint(&b), built(&b));
    // Touch A so B is the least recently used.
    assert!(cache.lookup("A", &fingerprint(&a)).is_some());
    cache.insert("C".into(), fingerprint(&c), built(&c));
    let stats = cache.stats();
    assert_eq!(stats.entries, 2);
    assert!(stats.retained_bytes <= one * 2 + one / 2);
    assert_eq!(stats.evictions, 1);
    assert!(cache.lookup("B", &fingerprint(&b)).is_none());
    assert!(cache.lookup("A", &fingerprint(&a)).is_some());
    assert!(cache.lookup("C", &fingerprint(&c)).is_some());
    // A catalogue larger than the whole budget is never retained.
    let mut tiny = CatalogueIndexCache::with_budget(one / 2);
    assert!(!tiny.insert("A".into(), fingerprint(&a), built(&a)));
    assert_eq!(tiny.stats().entries, 0);
}

#[test]
fn concurrent_requests_share_one_consistent_cache() {
    let dir = tempfile::tempdir().unwrap();
    let path = dat_file(dir.path(), "A", 50);
    let shared = CatalogueIndexCache::default().shared();
    let fp = fingerprint(&path);
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let shared = shared.clone();
            let fp = fp.clone();
            let path = path.clone();
            std::thread::spawn(move || {
                let hit = shared.lock().unwrap().lookup("A", &fp);
                let catalogue = match hit {
                    Some(catalogue) => catalogue,
                    None => {
                        // Built outside the lock, then offered to the cache.
                        let catalogue = built(&path);
                        shared
                            .lock()
                            .unwrap()
                            .insert("A".into(), fp.clone(), catalogue.clone());
                        catalogue
                    }
                };
                catalogue.total_roms
            })
        })
        .collect();
    for thread in threads {
        assert_eq!(thread.join().unwrap(), 50);
    }
    let stats = shared.lock().unwrap().stats();
    assert_eq!(stats.entries, 1);
    assert!(stats.retained_bytes > 0);
}

#[test]
fn a_large_catalogue_is_indexed_once_and_estimated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dat_file(dir.path(), "Large", 20_000);
    let catalogue = built(&path);
    assert_eq!(catalogue.total_roms, 20_000);
    assert!(catalogue.estimated_bytes > 20_000 * 256);
    let mut cache = CatalogueIndexCache::default();
    cache.insert("Large".into(), fingerprint(&path), catalogue);
    for _ in 0..5 {
        assert!(cache.lookup("Large", &fingerprint(&path)).is_some());
    }
    assert_eq!(cache.stats().hits, 5);
}

#[test]
fn a_same_size_dat_rewrite_that_restores_mtime_is_still_a_miss() {
    let dir = tempfile::tempdir().unwrap();
    let path = dat_file(dir.path(), "A", 3);
    let set_mtime = |path: &Path| {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000))
            .unwrap()
    };
    set_mtime(&path);
    let mut cache = CatalogueIndexCache::default();
    cache.insert("A".into(), fingerprint(&path), built(&path));
    std::thread::sleep(std::time::Duration::from_millis(20));
    // Same length, different content, mtime restored.
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("Game 1", "Game X");
    fs::write(&path, text).unwrap();
    set_mtime(&path);
    assert!(cache.lookup("A", &fingerprint(&path)).is_none());
}
