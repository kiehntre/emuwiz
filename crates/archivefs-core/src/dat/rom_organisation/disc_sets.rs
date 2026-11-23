//! Disc sets in a bulk Move (owner decision 2026-11-23).
//!
//! A disc set is a sheet (`.cue`, `.m3u`, `.m3u8`, `.gdi`) plus every file it
//! names, followed one level further (an `.m3u` listing `.cue` files brings
//! those cues' tracks too). The sheets name their files by plain file name,
//! so the only safe bulk change is to move every file of the set into the
//! same new folder with every name unchanged: each reference still resolves.
//!
//! - A set moves only when it is complete and unambiguous: every reference
//!   is a plain file name of an existing regular file (not a link) in the
//!   sheet's own folder, no file belongs to two sets, and every file is in
//!   the preview. Anything else blocks the sheet and every file it names, so
//!   a sheet is never moved away from its tracks (or the other way round).
//! - Every file of a set moves or none does: [`hold_disc_sets_together`]
//!   blocks the rest of a set when one file cannot move, and
//!   [`refuse_partial_disc_sets`] refuses an Apply that selects part of one.
//! - Renaming disc sets stays refused (see `rename_guard`).
//!
//! Detection only reads (bounded) sheet text and file metadata.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use super::model::{OrganisationPlan, OrganisationPlanEntry, OrganisationStatus};

/// Sheets larger than this are not read: no real cue, playlist or gdi is.
const MAX_SHEET_BYTES: u64 = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SheetKind {
    Cue,
    Playlist,
    Gdi,
}

/// The sheet kind of `path` by its extension (case-insensitive), if any.
pub(crate) fn sheet_kind(path: &Path) -> Option<SheetKind> {
    let extension = path.extension()?.to_string_lossy().to_ascii_lowercase();
    match extension.as_str() {
        "cue" => Some(SheetKind::Cue),
        "m3u" | "m3u8" => Some(SheetKind::Playlist),
        "gdi" => Some(SheetKind::Gdi),
        _ => None,
    }
}

/// The file names a sheet's text refers to, in order, exactly as written.
pub(crate) fn parse_references(kind: SheetKind, text: &str) -> Vec<String> {
    let lines = text
        .lines()
        .map(|line| line.trim_start_matches('\u{feff}').trim());
    match kind {
        SheetKind::Cue => lines.filter_map(cue_file_reference).collect(),
        SheetKind::Playlist => lines
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(str::to_string)
            .collect(),
        // The first line is the track count; each track line is
        // `number lba type sector_size filename offset`.
        SheetKind::Gdi => lines
            .filter(|line| !line.is_empty())
            .skip(1)
            .filter_map(|line| gdi_tokens(line).into_iter().nth(4))
            .collect(),
    }
}

/// `FILE "name" TYPE` or `FILE name TYPE`.
fn cue_file_reference(line: &str) -> Option<String> {
    let (keyword, rest) = line.split_once(char::is_whitespace)?;
    if !keyword.eq_ignore_ascii_case("FILE") {
        return None;
    }
    let rest = rest.trim();
    if let Some(quoted) = rest.strip_prefix('"') {
        return quoted.split_once('"').map(|(name, _)| name.to_string());
    }
    let name = match rest.rsplit_once(char::is_whitespace) {
        Some((name, _file_type)) => name.trim(),
        None => rest,
    };
    Some(name.to_string())
}

/// Whitespace-separated fields, a double-quoted field kept whole.
fn gdi_tokens(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut rest = line.trim();
    while !rest.is_empty() {
        if let Some(quoted) = rest.strip_prefix('"') {
            let (token, after) = quoted.split_once('"').unwrap_or((quoted, ""));
            tokens.push(token.to_string());
            rest = after.trim_start();
        } else {
            let (token, after) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            tokens.push(token.to_string());
            rest = after.trim_start();
        }
    }
    tokens
}

/// Whether a reference is a plain file name in the sheet's own folder: no
/// folder part of either kind, not `.`/`..`, not empty.
pub(crate) fn is_plain_reference(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && name != "."
        && name != ".."
        && !name.contains('\0')
}

/// The references of one sheet, read without following a link, bounded.
fn read_references(sheet: &Path, kind: SheetKind) -> Result<Vec<String>, String> {
    let name = display_name(sheet);
    let metadata = std::fs::symlink_metadata(sheet)
        .map_err(|error| format!("{name} could not be read ({error})"))?;
    if !metadata.is_file() {
        return Err(format!("{name} is not a regular file"));
    }
    if metadata.len() > MAX_SHEET_BYTES {
        return Err(format!("{name} is larger than 1 MiB, so it was not read"));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(sheet)
        .and_then(|file| file.take(MAX_SHEET_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("{name} could not be read ({error})"))?;
    if bytes.len() as u64 > MAX_SHEET_BYTES {
        return Err(format!("{name} is larger than 1 MiB, so it was not read"));
    }
    Ok(parse_references(kind, &String::from_utf8_lossy(&bytes)))
}

pub(crate) fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// One detected set: its top sheet and every file, sheet first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscSet {
    pub sheet: PathBuf,
    pub members: Vec<PathBuf>,
    /// Why the set cannot move as a whole, or `None` when it can.
    pub invalid: Option<String>,
}

/// The disc sets among one preview's files.
#[derive(Debug, Default)]
pub(crate) struct DiscSets {
    pub sets: Vec<DiscSet>,
    members: HashSet<PathBuf>,
}

impl DiscSets {
    /// Whether `path` is a file of any detected set (valid or not).
    pub(crate) fn contains(&self, path: &Path) -> bool {
        self.members.contains(path)
    }
}

/// Finds every disc set among `files` (the preview's candidates).
pub(crate) fn detect(files: &[&Path]) -> DiscSets {
    let in_preview: HashSet<&Path> = files.iter().copied().collect();
    let mut parsed: HashMap<PathBuf, Result<Vec<String>, String>> = HashMap::new();
    let mut parse = |sheet: &Path| -> Result<Vec<String>, String> {
        parsed
            .entry(sheet.to_path_buf())
            .or_insert_with(|| match sheet_kind(sheet) {
                Some(kind) => read_references(sheet, kind),
                None => Ok(Vec::new()),
            })
            .clone()
    };
    let sheets: Vec<&Path> = files
        .iter()
        .copied()
        .filter(|path| sheet_kind(path).is_some())
        .collect();
    // A sheet another preview sheet in its folder names is part of that set.
    let mut named_sheets: HashSet<PathBuf> = HashSet::new();
    for sheet in &sheets {
        let Some(folder) = sheet.parent() else {
            continue;
        };
        for reference in parse(sheet).unwrap_or_default() {
            let path = folder.join(&reference);
            if is_plain_reference(&reference)
                && path != *sheet
                && sheet_kind(&path).is_some()
                && in_preview.contains(path.as_path())
            {
                named_sheets.insert(path);
            }
        }
    }
    let mut sets = Vec::new();
    for sheet in sheets.iter().filter(|s| !named_sheets.contains(**s)) {
        sets.push(collect_set(sheet, &in_preview, &mut parse));
    }
    // A file of two sets cannot move with both: neither moves.
    let mut owners: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
    for (index, set) in sets.iter().enumerate() {
        for member in &set.members {
            owners.entry(member.clone()).or_default().push(index);
        }
    }
    for (member, owning) in &owners {
        if owning.len() < 2 {
            continue;
        }
        let names: Vec<String> = owning
            .iter()
            .map(|&i| display_name(&sets[i].sheet))
            .collect();
        for &index in owning {
            sets[index].invalid.get_or_insert_with(|| {
                format!(
                    "{} is named by more than one disc set ({})",
                    display_name(member),
                    names.join(", ")
                )
            });
        }
    }
    // Sheets named only from a loop of sheets have no top: never moved.
    let reached: HashSet<PathBuf> = sets.iter().flat_map(|s| s.members.clone()).collect();
    for sheet in named_sheets.iter().filter(|s| !reached.contains(*s)) {
        sets.push(DiscSet {
            sheet: sheet.clone(),
            members: vec![sheet.clone()],
            invalid: Some(format!(
                "{} is part of a loop of sheets naming each other",
                display_name(sheet)
            )),
        });
    }
    let members = sets.iter().flat_map(|s| s.members.clone()).collect();
    DiscSets { sets, members }
}

/// The set of one top sheet: its files, then the files of any sheet it names.
fn collect_set(
    sheet: &Path,
    in_preview: &HashSet<&Path>,
    parse: &mut dyn FnMut(&Path) -> Result<Vec<String>, String>,
) -> DiscSet {
    let mut set = DiscSet {
        sheet: sheet.to_path_buf(),
        members: vec![sheet.to_path_buf()],
        invalid: None,
    };
    let fail = |set: &mut DiscSet, reason: String| {
        set.invalid.get_or_insert(reason);
    };
    let mut nested = Vec::new();
    // Level 1: the top sheet's references; level 2: those of a named sheet.
    match parse(sheet) {
        Err(reason) => fail(&mut set, reason),
        Ok(references) if references.is_empty() => {
            fail(&mut set, format!("{} names no files", display_name(sheet)))
        }
        Ok(references) => {
            for reference in references {
                match resolve(sheet, &reference, in_preview) {
                    Ok(path) => {
                        if sheet_kind(&path).is_some() && !nested.contains(&path) {
                            nested.push(path.clone());
                        }
                        if !set.members.contains(&path) {
                            set.members.push(path);
                        }
                    }
                    Err(reason) => fail(&mut set, reason),
                }
            }
        }
    }
    for inner in nested {
        match parse(&inner) {
            Err(reason) => fail(&mut set, reason),
            Ok(references) if references.is_empty() => {
                fail(&mut set, format!("{} names no files", display_name(&inner)))
            }
            Ok(references) => {
                for reference in references {
                    match resolve(&inner, &reference, in_preview) {
                        Ok(path) if sheet_kind(&path).is_some() => fail(
                            &mut set,
                            format!(
                                "{} names another sheet ({}); only one level of sheets is moved together",
                                display_name(&inner),
                                display_name(&path)
                            ),
                        ),
                        Ok(path) => {
                            if !set.members.contains(&path) {
                                set.members.push(path);
                            }
                        }
                        Err(reason) => fail(&mut set, reason),
                    }
                }
            }
        }
    }
    set
}

/// The file `reference` names beside `sheet`, when it is safe to move.
fn resolve(sheet: &Path, reference: &str, in_preview: &HashSet<&Path>) -> Result<PathBuf, String> {
    let sheet_name = display_name(sheet);
    if !is_plain_reference(reference) {
        return Err(format!(
            "{sheet_name} refers to \"{reference}\", which is not a plain file name in its own folder"
        ));
    }
    let folder = sheet.parent().ok_or("the sheet has no folder")?;
    let path = folder.join(reference);
    match std::fs::symlink_metadata(&path) {
        Err(_) => Err(format!("{reference}, named by {sheet_name}, is missing")),
        Ok(meta) if meta.file_type().is_symlink() => Err(format!(
            "{reference}, named by {sheet_name}, is a shortcut (symlink), not a real file"
        )),
        Ok(meta) if !meta.is_file() => Err(format!(
            "{reference}, named by {sheet_name}, is not a regular file"
        )),
        Ok(_) if !in_preview.contains(path.as_path()) => Err(format!(
            "{reference}, named by {sheet_name}, is not in this preview, so the set cannot move complete"
        )),
        Ok(_) => Ok(path),
    }
}

/// Planner post-pass (Move mode, before collision checks): gives every file
/// of a valid set the same platform folder with its own name, or blocks the
/// whole set with the reason.
pub(crate) fn settle(sets: &DiscSets, master_root: &Path, entries: &mut [OrganisationPlanEntry]) {
    let index: HashMap<PathBuf, usize> = entries
        .iter()
        .enumerate()
        .map(|(i, entry)| (entry.source_path.clone(), i))
        .collect();
    for set in &sets.sets {
        let members: Vec<usize> = set
            .members
            .iter()
            .filter_map(|path| index.get(path).copied())
            .collect();
        let sheet_name = display_name(&set.sheet);
        if let Some(reason) = &set.invalid {
            block_set(
                entries,
                &members,
                &format!(
                    "This file belongs to the disc set of {sheet_name}, which cannot be moved as a whole: {reason}. Nothing will change for this file."
                ),
            );
            continue;
        }
        match destination_of(set, &members, master_root, entries) {
            Err(reason) => {
                for &i in &members {
                    entries[i].disc_set = Some(set.sheet.clone());
                }
                block_set(
                    entries,
                    &members,
                    &format!(
                        "This file belongs to the disc set of {sheet_name}, which moves only as a whole: {reason}. Nothing will change for this file."
                    ),
                );
            }
            Ok(target) => {
                for &i in &members {
                    let entry = &mut entries[i];
                    let name = entry.source_path.file_name().unwrap_or_default();
                    entry.destination_path = target.folder.join(name);
                    if entry.platform.is_none() {
                        entry.platform = Some(target.platform.clone());
                        entry.platform_display_name = target.display_name.clone();
                        entry.platform_source = format!("Disc set: same as {sheet_name}");
                    }
                    entry.layout_folder = Some(target.layout_folder.clone());
                    entry.status = if entry.source_path.parent() == Some(target.folder.as_path()) {
                        OrganisationStatus::AlreadyOrganised
                    } else {
                        OrganisationStatus::Suggested
                    };
                    entry.reason = None;
                    entry.disc_set = Some(set.sheet.clone());
                }
            }
        }
    }
}

struct SetTarget {
    folder: PathBuf,
    layout_folder: String,
    platform: String,
    display_name: String,
}

/// The one folder a valid set moves into, or why it cannot move.
fn destination_of(
    set: &DiscSet,
    members: &[usize],
    master_root: &Path,
    entries: &[OrganisationPlanEntry],
) -> Result<SetTarget, String> {
    let mut platforms: BTreeMap<String, String> = BTreeMap::new();
    for &i in members {
        let entry = &entries[i];
        match entry.status {
            OrganisationStatus::Suggested | OrganisationStatus::AlreadyOrganised => {
                if let Some(platform) = &entry.platform {
                    platforms
                        .entry(platform.clone())
                        .or_insert_with(|| entry.platform_display_name.clone());
                }
            }
            // No platform of its own: it takes the set's.
            OrganisationStatus::Blocked
                if entry.reason.as_deref() == Some(super::plan::NO_PLATFORM_REASON) => {}
            _ => {
                return Err(format!(
                    "{} cannot move ({})",
                    display_name(&entry.source_path),
                    entry.reason.as_deref().unwrap_or(entry.status.label())
                ));
            }
        }
        let name = display_name(&entry.source_path);
        if !crate::dat::rename_apply::preflight::is_safe_basename(&name) {
            return Err(format!("{name} is not a safe file name"));
        }
    }
    let (platform, system_name) = match platforms.len() {
        0 => {
            return Err(format!(
                "no platform identity could be resolved for any file of it ({})",
                display_name(&set.sheet)
            ));
        }
        1 => platforms.into_iter().next().expect("one platform"),
        _ => {
            return Err(format!(
                "the files of this disc set resolve to different systems ({})",
                platforms.into_values().collect::<Vec<_>>().join(", ")
            ));
        }
    };
    let layout_folder = super::plan::layout_folder_in(master_root, &platform)?;
    Ok(SetTarget {
        folder: master_root.join(&layout_folder),
        layout_folder,
        platform,
        display_name: system_name,
    })
}

/// Blocks every movable file of a set (a file already blocked for its own
/// reason keeps it, except one only missing a platform).
fn block_set(entries: &mut [OrganisationPlanEntry], members: &[usize], reason: &str) {
    for &i in members {
        let entry = &mut entries[i];
        let no_platform = entry.status == OrganisationStatus::Blocked
            && entry.reason.as_deref() == Some(super::plan::NO_PLATFORM_REASON);
        if entry.status == OrganisationStatus::Suggested || no_platform {
            entry.status = OrganisationStatus::Blocked;
            entry.reason = Some(reason.to_string());
        }
    }
}

/// Every file of a set moves or none does: when one file of a set cannot
/// move (a conflict, a guard refusal, ...), its Suggested siblings become
/// Blocked with the reason. Returns how many entries it blocked.
pub fn hold_disc_sets_together(entries: &mut [OrganisationPlanEntry]) -> usize {
    let mut groups: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
    for (i, entry) in entries.iter().enumerate() {
        if let Some(sheet) = &entry.disc_set {
            groups.entry(sheet.clone()).or_default().push(i);
        }
    }
    let mut blocked = 0;
    for (sheet, members) in groups {
        let Some(&stuck) = members
            .iter()
            .find(|&&i| entries[i].status != OrganisationStatus::Suggested)
        else {
            continue;
        };
        let why = format!(
            "This file moves only with its whole disc set ({}), and {} cannot move ({}). Nothing will change for this file.",
            display_name(&sheet),
            display_name(&entries[stuck].source_path),
            entries[stuck]
                .reason
                .as_deref()
                .unwrap_or(entries[stuck].status.label())
        );
        for &i in &members {
            if entries[i].status == OrganisationStatus::Suggested {
                entries[i].status = OrganisationStatus::Blocked;
                entries[i].reason = Some(why.clone());
                blocked += 1;
            }
        }
    }
    blocked
}

/// Apply (every path, before any change): a disc set is selected whole or
/// not at all.
pub fn refuse_partial_disc_sets(
    plan: &OrganisationPlan,
    approved: &BTreeSet<String>,
) -> Result<(), String> {
    let mut groups: BTreeMap<&Path, (usize, usize)> = BTreeMap::new();
    for entry in &plan.entries {
        let Some(sheet) = entry.disc_set.as_deref() else {
            continue;
        };
        let counts = groups.entry(sheet).or_default();
        counts.0 += 1;
        if entry.status == OrganisationStatus::Suggested
            && approved.contains(entry.source_path.to_string_lossy().as_ref())
        {
            counts.1 += 1;
        }
    }
    for (sheet, (total, selected)) in groups {
        if selected > 0 && selected != total {
            let files = total - 1;
            return Err(format!(
                "Move the whole disc set together: {} and its {files} file{} must all be selected. Nothing was changed.",
                display_name(sheet),
                if files == 1 { "" } else { "s" }
            ));
        }
    }
    Ok(())
}

/// The top sheet of `source`'s set in `plan`, and every file of that set.
pub fn disc_set_of<'a>(
    plan: &'a OrganisationPlan,
    source: &Path,
) -> Option<(&'a Path, Vec<&'a Path>)> {
    let sheet = plan
        .entries
        .iter()
        .find(|entry| entry.source_path == source)?
        .disc_set
        .as_deref()?;
    let members = plan
        .entries
        .iter()
        .filter(|entry| entry.disc_set.as_deref() == Some(sheet))
        .map(|entry| entry.source_path.as_path())
        .collect();
    Some((sheet, members))
}
