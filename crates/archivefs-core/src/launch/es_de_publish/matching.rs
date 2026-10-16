//! Deciding whether a new playing-library entry is already in an existing
//! gamelist, without ever editing existing entries.
//!
//! ES-DE resolves a gamelist `<path>` starting with `./` against the system's
//! ROM directory (its `es_systems.xml` `<path>`, where `%ROMPATH%` is the
//! `ROMDirectory` setting), not against the gamelist's own directory. `~/`
//! paths resolve against a home EmuWiz does not reliably know here, and other
//! relative forms are not resolved by ES-DE at all, so both stay unresolved.
//!
//! An entry is "already present" only by exact (lexically normalised) path or
//! by file identity (device + inode) with the destination or its source ROM.
//! A shared file name is never a match. It only triggers a per-entry refusal
//! when an existing entry with that name cannot be resolved or points at a
//! missing file (the game may have moved), because adding a second entry could
//! strand the user's metadata on the old one.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;

const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;

/// Lexically normalised absolute path (repeated separators and `.` removed).
/// `None` for relative, `~`-prefixed or `..`-containing paths.
pub(super) fn lexical_absolute(text: &str) -> Option<PathBuf> {
    let path = Path::new(text);
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return None;
    }
    Some(path.components().collect())
}

/// Device + inode of the file a path finally refers to.
type Identity = (u64, u64);

/// Identity of the regular file `path` names, following a final-component
/// symlink for reading only. A link that changes while it is read, or that
/// names anything but a regular file, establishes no identity.
#[cfg(unix)]
fn identity(path: &Path) -> std::io::Result<Identity> {
    use std::os::unix::fs::MetadataExt;
    let link = |path: &Path| -> std::io::Result<Option<PathBuf>> {
        let metadata = fs::symlink_metadata(path)?;
        Ok(if metadata.file_type().is_symlink() {
            Some(fs::read_link(path)?)
        } else {
            None
        })
    };
    let before = link(path)?;
    let metadata = fs::metadata(path)?;
    if link(path)? != before {
        return Err(std::io::Error::other(
            "the link changed while it was checked",
        ));
    }
    if !metadata.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    // Some FUSE filesystems report inode 0; that is no identity at all.
    if metadata.ino() == 0 {
        return Err(std::io::Error::other("file has no stable identity"));
    }
    Ok((metadata.dev(), metadata.ino()))
}
#[cfg(not(unix))]
fn identity(_: &Path) -> std::io::Result<Identity> {
    Err(std::io::Error::other(
        "file identity is unsupported on this host",
    ))
}

/// The verified, canonical ROM directory for one ES-DE system, or why there
/// is none. Never the gamelist's directory.
pub(super) fn system_rom_root(
    profile: &EsDeProfile,
    es_de_system: &str,
) -> Result<PathBuf, &'static str> {
    use crate::emulator_environment::es_de::PathResolutionState;
    let mut findings = profile
        .systems
        .iter()
        .filter(|s| s.name.as_deref() == Some(es_de_system));
    let first = findings
        .next()
        .ok_or("the system has no ES-DE definition")?;
    // Several definitions (custom plus bundled) must agree; ES-DE's override
    // order is not modelled here.
    if findings.any(|s| s.rom_path_raw != first.rom_path_raw) {
        return Err("the system's ROM path is defined more than once");
    }
    let root = match (first.rom_path_resolution, &first.rom_path_resolved) {
        (PathResolutionState::Resolved, Some(path)) if !path.lossy => PathBuf::from(&path.display),
        (PathResolutionState::ContainsUnexpandedVariable, _) => {
            let raw = first.rom_path_raw.as_deref().unwrap_or_default();
            let rest = raw
                .strip_prefix("%ROMPATH%")
                .ok_or("the system's ROM path uses an unsupported variable")?;
            if rest.contains('%') {
                return Err("the system's ROM path uses an unsupported variable");
            }
            let base = rom_directory_setting(profile)?;
            let rest = rest.trim_start_matches('/');
            if rest.is_empty() {
                base
            } else {
                base.join(rest)
            }
        }
        _ => return Err("the system's ROM path is not configured or not absolute"),
    };
    let text = root.to_str().ok_or("the ROM path is not valid UTF-8")?;
    let lexical = lexical_absolute(text).ok_or("the ROM path is relative or contains '..'")?;
    let canonical = fs::canonicalize(&lexical).map_err(|_| "the ROM directory does not exist")?;
    if !fs::metadata(&canonical).is_ok_and(|m| m.is_dir()) {
        return Err("the ROM path is not a directory");
    }
    Ok(canonical)
}

/// `ROMDirectory` from the profile's `es_settings.xml`. Only a single
/// absolute value is accepted: an empty value means ES-DE's `~/ROMs` default,
/// and `~` refers to a home this module does not know.
fn rom_directory_setting(profile: &EsDeProfile) -> Result<PathBuf, &'static str> {
    use quick_xml::events::Event;
    if profile.settings_file.path.lossy {
        return Err("the ES-DE settings path is not valid UTF-8");
    }
    let path = PathBuf::from(&profile.settings_file.path.display);
    let text = bounded_text(&path, MAX_SETTINGS_BYTES)
        .map_err(|_| "the ES-DE settings file cannot be read")?;
    let mut reader = quick_xml::Reader::from_str(&text);
    let mut values = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) if e.name().as_ref() == b"string" => {
                let mut name = None;
                let mut value = None;
                for attr in e.attributes() {
                    let attr = attr.map_err(|_| "the ES-DE settings file is malformed")?;
                    let decoded = attr
                        .normalized_value(quick_xml::XmlVersion::Explicit1_0)
                        .map_err(|_| "the ES-DE settings file is malformed")?
                        .into_owned();
                    match attr.key.as_ref() {
                        b"name" => name = Some(decoded),
                        b"value" => value = Some(decoded),
                        _ => {}
                    }
                }
                if name.as_deref() == Some("ROMDirectory") {
                    values.push(value.unwrap_or_default());
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return Err("the ES-DE settings file is malformed"),
        }
    }
    match values.as_slice() {
        [value] if value.starts_with('/') => Ok(PathBuf::from(value.trim_end_matches('/'))
            .components()
            .collect()),
        [value] if value.is_empty() || value.starts_with('~') => {
            Err("ROMDirectory uses the home directory, which EmuWiz cannot verify here")
        }
        [] => Err("ROMDirectory is not set, so ES-DE's home-based default applies"),
        [_] => Err("ROMDirectory is not an absolute path"),
        _ => Err("ROMDirectory is set more than once"),
    }
}

/// Every file identity a preview's matching decisions relied on, re-checked
/// under the publication lock before Apply writes. A change (a retargeted,
/// broken or replaced link, or a moved ROM directory component) makes the
/// preview stale.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MatchEvidence {
    /// path -> (identity or `None` when absent, canonical parent for `./` entries)
    files: BTreeMap<PathBuf, (Option<Identity>, Option<PathBuf>)>,
    /// Identities of this plan's source ROMs. Creating the plan's own
    /// playing-library links between preview and Apply turns an absent path
    /// into one of these; that is the only change accepted.
    planned_sources: BTreeSet<Identity>,
}

impl MatchEvidence {
    fn record(&mut self, path: &Path, identity: Option<Identity>, parent: Option<PathBuf>) {
        self.files.insert(path.to_path_buf(), (identity, parent));
    }

    pub(super) fn verify(&self) -> Result<(), String> {
        for (path, (expected, parent)) in &self.files {
            if let Some(parent) = parent {
                let current = path.parent().and_then(|p| fs::canonicalize(p).ok());
                if current.as_ref() != Some(parent) {
                    return Err(format!("{} moved since preview", path.display()));
                }
            }
            let current = match identity(path) {
                Ok(identity) => Some(identity),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(format!("{}: {error}", path.display())),
            };
            let planned_link =
                expected.is_none() && current.is_some_and(|id| self.planned_sources.contains(&id));
            if current != *expected && !planned_link {
                return Err(format!("{} changed since preview", path.display()));
            }
        }
        Ok(())
    }
}

/// How one existing `<game>` path resolved.
enum Resolution {
    /// A concrete path; `identity` is `None` when its file does not exist.
    Path {
        lexical: PathBuf,
        identity: Option<Identity>,
        /// Canonical parent, for `./` entries whose containment was checked.
        parent: Option<PathBuf>,
    },
    Unresolved(&'static str),
}

/// What to do with one new entry.
pub(super) enum Decision {
    Add,
    AlreadyPresent,
    Refuse {
        existing: String,
        reason: &'static str,
    },
}

pub(super) struct ExistingEntries {
    pub(super) evidence: MatchEvidence,
    exact: BTreeSet<PathBuf>,
    identities: BTreeSet<Identity>,
    by_file_name: BTreeMap<OsString, Vec<(String, Resolution)>>,
}

impl ExistingEntries {
    pub(super) fn index(
        paths: Option<&BTreeSet<String>>,
        root: &Result<PathBuf, &'static str>,
    ) -> Self {
        let mut evidence = MatchEvidence::default();
        let mut exact = BTreeSet::new();
        let mut identities = BTreeSet::new();
        let mut by_file_name: BTreeMap<OsString, Vec<_>> = BTreeMap::new();
        for raw in paths.into_iter().flatten() {
            // Whether ES-DE trims `<path>` text is not established here, so a
            // padded path is neither a match nor ignored: it is keyed by its
            // trimmed file name and refuses that game.
            let trimmed = raw.trim();
            let resolution = if trimmed != raw {
                Resolution::Unresolved("its path has leading or trailing whitespace")
            } else {
                resolve(raw, root)
            };
            if let Resolution::Path {
                lexical,
                identity,
                parent,
            } = &resolution
            {
                evidence.record(lexical, *identity, parent.clone());
                exact.insert(lexical.clone());
                if let Some(identity) = identity {
                    identities.insert(*identity);
                }
            }
            if let Some(name) = Path::new(trimmed).file_name() {
                by_file_name
                    .entry(name.to_owned())
                    .or_default()
                    .push((raw.clone(), resolution));
            }
        }
        Self {
            evidence,
            exact,
            identities,
            by_file_name,
        }
    }

    pub(super) fn decide(&mut self, destination: &Path, source: &Path) -> Decision {
        if self.exact.contains(destination) {
            return Decision::AlreadyPresent;
        }
        let mut known = Vec::new();
        for path in [destination, source] {
            let found = identity(path).ok();
            self.evidence.record(path, found, None);
            known.extend(found);
        }
        if let Ok(id) = identity(source) {
            self.evidence.planned_sources.insert(id);
        }
        if known.iter().any(|id| self.identities.contains(id)) {
            return Decision::AlreadyPresent;
        }
        let Some(candidates) = destination
            .file_name()
            .and_then(|name| self.by_file_name.get(name))
        else {
            return Decision::Add;
        };
        for (raw, resolution) in candidates {
            let reason = match resolution {
                Resolution::Unresolved(reason) => reason,
                Resolution::Path { identity: None, .. } => {
                    "its file no longer exists, so the game may have moved"
                }
                // A present file with the same name and a different identity
                // is a different game, but only when this game's identity is
                // known at all.
                Resolution::Path {
                    identity: Some(_), ..
                } if known.is_empty() => "this game's file cannot be checked against it",
                Resolution::Path { .. } => continue,
            };
            return Decision::Refuse {
                existing: raw.clone(),
                reason,
            };
        }
        Decision::Add
    }
}

fn resolve(raw: &str, root: &Result<PathBuf, &'static str>) -> Resolution {
    let lexical = if let Some(relative) = raw.strip_prefix("./") {
        let root = match root {
            Ok(root) => root,
            Err(reason) => return Resolution::Unresolved(reason),
        };
        let relative = Path::new(relative);
        if relative.as_os_str().is_empty()
            || relative.components().any(|c| {
                !matches!(
                    c,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            })
        {
            return Resolution::Unresolved("its relative path leaves the ROM directory");
        }
        root.join(relative).components().collect::<PathBuf>()
    } else if raw.starts_with('/') {
        match lexical_absolute(raw) {
            Some(path) => path,
            None => return Resolution::Unresolved("its path contains '..'"),
        }
    } else if raw.starts_with('~') {
        return Resolution::Unresolved(
            "its path is relative to a home directory EmuWiz cannot verify",
        );
    } else {
        return Resolution::Unresolved("its path is neither './'-relative nor absolute");
    };
    // Directory containment is checked on the canonical parent: a directory
    // symlink leaving the ROM root is refused. The leaf may be a symlink (the
    // normal EmuWiz/RetroDECK link layout); it is followed only to read the
    // identity of the file it names, never to write.
    let mut checked_parent = None;
    if raw.starts_with("./") {
        let Some(parent) = lexical.parent() else {
            return Resolution::Unresolved("its relative path leaves the ROM directory");
        };
        match fs::canonicalize(parent) {
            Ok(canonical) if root.as_ref().is_ok_and(|root| canonical.starts_with(root)) => {
                checked_parent = Some(canonical);
            }
            Ok(_) => {
                return Resolution::Unresolved(
                    "it resolves outside the ROM directory through a directory symlink",
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Resolution::Path {
                    lexical,
                    identity: None,
                    parent: None,
                };
            }
            Err(_) => return Resolution::Unresolved("its file cannot be checked"),
        }
    }
    let found = identity(&lexical);
    // The directory chain must not have changed while the leaf was read.
    if let Some(expected) = &checked_parent {
        if lexical
            .parent()
            .and_then(|p| fs::canonicalize(p).ok())
            .as_ref()
            != Some(expected)
        {
            return Resolution::Unresolved("its directory changed while it was checked");
        }
    }
    match found {
        Ok(identity) => Resolution::Path {
            lexical,
            identity: Some(identity),
            parent: checked_parent,
        },
        // A broken symlink is also NotFound; it is "gone" and has no identity.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Resolution::Path {
            lexical,
            identity: None,
            parent: checked_parent,
        },
        Err(_) => Resolution::Unresolved("its file cannot be checked"),
    }
}
