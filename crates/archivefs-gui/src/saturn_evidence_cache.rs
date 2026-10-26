//! Display-only Saturn evidence cache. Content inspection belongs to workers;
//! the UI checks only file identity/metadata. Apply must still revalidate bytes.
use archivefs_core::{
    ingestion::cue_bin::{CueError, resolve_cue, resolve_cue_all_files_lenient},
    mame_collection_analyser::inspection::SourceStamp,
};
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};

const MAX_WATCHED_PATHS: usize = 512;
struct Sources {
    stamps: BTreeMap<PathBuf, Option<SourceStamp>>,
    links: BTreeMap<PathBuf, PathBuf>,
    limit_exceeded: bool,
}
impl Sources {
    fn record(&mut self, path: &Path) {
        if self.stamps.len() >= MAX_WATCHED_PATHS && !self.stamps.contains_key(path) {
            self.limit_exceeded = true;
            return;
        }
        self.stamps
            .entry(path.to_owned())
            .or_insert_with(|| SourceStamp::observe(path).ok());
    }
    fn add(&mut self, path: &Path) {
        self.record(path);
        if let Some(parent) = path.parent() {
            // Directory identity catches newly created missing/case-ambiguous
            // companions, including references absent from an incomplete manifest.
            self.record(parent);
        }
    }
    fn reference(&mut self, path: &Path, base: &Path) {
        if path.as_os_str().len() > 4096 || path.components().count() > 64 {
            self.limit_exceeded = true;
            return;
        }
        if !path.starts_with(base)
            || path.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
        {
            return;
        }
        // Walk filesystem aliases one hop at a time. canonicalize alone drops
        // intermediate links, whose retargeting must invalidate the snapshot.
        let mut pending: VecDeque<_> = path
            .strip_prefix(base)
            .unwrap()
            .components()
            .map(|c| c.as_os_str().to_owned())
            .collect();
        let mut current = base.to_owned();
        self.record(base);
        let mut hops = 0;
        let mut steps = 0;
        while let Some(component) = pending.pop_front() {
            steps += 1;
            if steps > 256 {
                self.limit_exceeded = true;
                return;
            }
            if component == "." {
                continue;
            }
            if component == ".." {
                if current == base || !current.pop() {
                    self.limit_exceeded = true;
                    return;
                }
                continue;
            }
            current.push(component);
            self.record(&current);
            if let Ok(target) = std::fs::read_link(&current) {
                hops += 1;
                if hops > 32 {
                    self.limit_exceeded = true;
                    return;
                }
                self.links
                    .entry(current.clone())
                    .or_insert_with(|| target.clone());
                current.pop();
                let relative = if target.is_absolute() {
                    current = base.to_owned();
                    match target.strip_prefix(base) {
                        Ok(relative) => relative,
                        Err(_) => {
                            self.limit_exceeded = true;
                            return;
                        }
                    }
                } else {
                    &target
                };
                for part in relative.components().rev() {
                    pending.push_front(part.as_os_str().to_owned());
                }
            }
        }
    }

    fn disc(source: &Path, patch: Option<&Path>) -> Self {
        let mut sources = Self {
            stamps: BTreeMap::new(),
            links: BTreeMap::new(),
            limit_exceeded: false,
        };
        sources.add(source);
        if let Some(patch) = patch {
            sources.add(patch);
        }
        if sources.regular(source)
            && source
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("cue"))
        {
            let base = source.parent().expect("absolute source parent");
            // The canonical raw-reference reader retains paths even when the
            // strict resolver refuses a missing/nonregular companion.
            if let Ok(sheet) = resolve_cue(source) {
                for path in sheet.referenced_paths {
                    sources.reference(&path, base);
                }
            }
            if let Ok(files) = resolve_cue_all_files_lenient(source) {
                for file in files {
                    match file {
                        Ok(path) | Err(CueError::MissingDataFile(path)) => {
                            sources.reference(&path, base)
                        }
                        _ => {}
                    }
                }
            }
        }
        sources
    }
    fn regular(&self, path: &Path) -> bool {
        self.stamps
            .get(path)
            .and_then(Option::as_ref)
            .is_some_and(|s| !s.directory)
    }
    fn current(&self) -> bool {
        self.stamps
            .iter()
            .all(|(path, stamp)| SourceStamp::observe(path).ok() == *stamp)
            && self
                .links
                .iter()
                .all(|(path, target)| std::fs::read_link(path).ok().as_ref() == Some(target))
    }
}

pub(crate) struct Observed<T> {
    sources: Sources,
    value: Result<T, String>,
}
impl<T> Observed<T> {
    /// Called in a worker, capturing descriptor/companion/patch identities
    /// before inspection. Changes during inspection cannot enter the cache.
    pub(crate) fn inspect(
        source: &Path,
        patch: Option<&Path>,
        load: impl FnOnce() -> Result<T, String>,
    ) -> Self {
        let sources = Sources::disc(source, patch);
        let value = if sources.limit_exceeded {
            Err("Disc evidence exceeds the bounded cache watch set; source preflight is unavailable.".into())
        } else if !sources.regular(source) {
            Err("Disc manifest unavailable: source is missing or unsafe; choose a regular disc descriptor.".into())
        } else {
            load()
        };
        Self { sources, value }
    }
}

struct Pending<K, T> {
    key: K,
    revision: u64,
    receiver: Receiver<Observed<T>>,
}

pub(crate) struct Cache<K, T> {
    key: Option<K>,
    revision: u64,
    entry: Option<Observed<T>>,
    pending: Option<Pending<K, T>>,
    pub(crate) error: Option<String>,
}
impl<K, T> Default for Cache<K, T> {
    fn default() -> Self {
        Self {
            key: None,
            revision: 0,
            entry: None,
            pending: None,
            error: None,
        }
    }
}
impl<K: Clone + Eq, T: Send + 'static> Cache<K, T> {
    pub(crate) fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.entry = None;
        self.error = None;
    }
    pub(crate) fn update(&mut self, key: &K) {
        if self.key.as_ref() != Some(key) {
            self.key = Some(key.clone());
            self.invalidate();
        }
        if self
            .entry
            .as_ref()
            .is_some_and(|entry| !entry.sources.current())
        {
            self.invalidate();
        }
        if let Some(pending) = &self.pending {
            match pending.receiver.try_recv() {
                Ok(observed) => {
                    let pending = self.pending.take().unwrap();
                    self.accept(pending.key, pending.revision, observed);
                }
                Err(TryRecvError::Disconnected) => {
                    let pending = self.pending.take().unwrap();
                    if self.key.as_ref() == Some(&pending.key) && self.revision == pending.revision
                    {
                        self.error = Some("Inspection stopped. No current source evidence is available; inspect again.".into());
                    }
                }
                Err(TryRecvError::Empty) => {}
            }
        }
    }
    fn accept(&mut self, key: K, revision: u64, observed: Observed<T>) {
        if self.key.as_ref() == Some(&key)
            && self.revision == revision
            && observed.sources.current()
        {
            self.entry = Some(observed);
        }
    }
    pub(crate) fn value(&self) -> Option<&Result<T, String>> {
        self.entry.as_ref().map(|entry| &entry.value)
    }
    pub(crate) fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub(crate) fn start(
        &mut self,
        key: K,
        context: eframe::egui::Context,
        load: impl FnOnce() -> Observed<T> + Send + 'static,
    ) -> bool {
        if self.busy()
            || self.value().is_some()
            || self.error.is_some()
            || self.key.as_ref() != Some(&key)
        {
            return false;
        }
        let (sender, receiver) = mpsc::channel();
        self.pending = Some(Pending {
            key,
            revision: self.revision,
            receiver,
        });
        std::thread::spawn(move || {
            let result = load();
            let _ = sender.send(result);
            context.request_repaint();
        });
        true
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
impl<K: Clone + Eq, T: Send + 'static> Cache<K, T> {
    pub(crate) fn finish_for_test(&mut self) {
        let Some(pending) = self.pending.take() else {
            assert!(
                self.value().is_some() || self.error.is_some(),
                "inspection result"
            );
            return;
        };
        let observed = pending
            .receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("worker completed");
        self.accept(pending.key, pending.revision, observed);
    }
}
