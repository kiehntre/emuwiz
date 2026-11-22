//! Turns "a launch began", "this process was started" and "the launch is
//! over" into session records, for a caller (Blue) that does its own launching.
//!
//! Every call does file work (discovery, the journal) and must run off the
//! drawing thread. Nothing here starts, stops or watches an emulator: it is
//! told what happened by the launcher and looks at saves at the two ends.

use super::discover::{DiscoveryOptions, DiscoveryRoots, discover};
use super::model::DiscoveredSave;
use super::session::{
    ExitObservation, FolderMark, NotedPlaces, ProcFs, ProcessTable, SaveMark, SaveTracking,
    SessionGame, SessionJournal, SessionLaunch, SessionRecord, SessionState,
};
use super::{ContinuityError, now_unix};
use crate::launch::process_log::{ExitSummary, StartedProcess};
use std::path::{Path, PathBuf};

/// Flatpak app ids of the emulators whose saves discovery knows.
const FLATPAK_IDS: [(&str, &str); 6] = [
    ("net.pcsx2.PCSX2", "PCSX2"),
    ("org.libretro.RetroArch", "RetroArch"),
    ("org.duckstation.DuckStation", "DuckStation"),
    ("org.DolphinEmu.dolphin-emu", "Dolphin"),
    ("net.rpcs3.RPCS3", "RPCS3"),
    ("org.ppsspp.PPSSPP", "PPSSPP"),
];

/// Executable names (lowercase, start of the file name) of native builds.
const NATIVE_NAMES: [(&str, &str); 6] = [
    ("pcsx2", "PCSX2"),
    ("retroarch", "RetroArch"),
    ("duckstation", "DuckStation"),
    ("dolphin-emu", "Dolphin"),
    ("rpcs3", "RPCS3"),
    ("ppsspp", "PPSSPP"),
];

/// What a started process is, from its executable and first arguments.
/// `through_launcher` is true when the process EmuWiz holds is not the
/// emulator itself, so its exit status is not the emulator's.
pub fn classify_started(executable: &Path, arguments: &[String]) -> SessionLaunch {
    let name = executable
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let flatpak_app = |texts: &mut dyn Iterator<Item = &str>| {
        for text in texts {
            if let Some((_, emulator)) = FLATPAK_IDS.iter().find(|(id, _)| text.contains(id)) {
                return Some((*emulator).to_string());
            }
        }
        None
    };
    let launch = |emulator: String, profile: &str, through_launcher: bool| SessionLaunch {
        emulator,
        profile: profile.to_string(),
        core: None,
        emulator_version: None,
        through_launcher,
    };
    if name == "flatpak" {
        let emulator = flatpak_app(&mut arguments.iter().map(String::as_str))
            .unwrap_or_else(|| "Unknown".to_string());
        return launch(emulator, "flatpak", true);
    }
    if let Some((_, emulator)) = NATIVE_NAMES
        .iter()
        .find(|(prefix, _)| name.starts_with(prefix))
    {
        // A file called like the emulator may still be a small script that
        // hands over to `flatpak run` (a common ~/.local/bin wrapper).
        // If the file cannot be read, what it is cannot be told: treated as
        // a launcher, so its exit status is never taken for the emulator's.
        // Only the start of the file is looked at, however large it is.
        let mut bytes = [0u8; 1024];
        let read = std::fs::File::open(executable).and_then(|mut file| {
            use std::io::Read;
            let mut filled = 0;
            while filled < bytes.len() {
                match file.read(&mut bytes[filled..])? {
                    0 => break,
                    more => filled += more,
                }
            }
            Ok(filled)
        });
        let Ok(filled) = read else {
            return launch((*emulator).to_string(), "native", true);
        };
        let head = String::from_utf8_lossy(&bytes[..filled]).into_owned();
        if head.starts_with("#!") {
            return match flatpak_app(&mut head.lines()) {
                Some(emulator) => launch(emulator, "flatpak", true),
                None => launch((*emulator).to_string(), "native", true),
            };
        }
        return launch((*emulator).to_string(), "native", false);
    }
    launch(
        executable
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Unknown".to_string()),
        "native",
        // Not a program EmuWiz recognises as an emulator itself: it may be
        // a launcher (`gamemoderun`, `env`, a script), so its exit status is
        // never taken for an emulator's.
        true,
    )
}

fn mark_of(save: &DiscoveredSave) -> Option<SaveMark> {
    Some(SaveMark {
        save_id: save.id.clone(),
        label: match (&save.identity.title, &save.entry_name) {
            (Some(title), Some(name)) => format!("{title} [{name}]"),
            (None, Some(name)) => name.clone(),
            _ => save
                .source_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
        },
        content_sha256: save.content_sha256.clone()?,
        last_changed_unix: None,
        before_launch: None,
        folder: None,
        path: Some(save.source_path.clone()),
    })
}

/// When `path` last changed on disk: the later of the modification and
/// status-change times, rounded up to a whole second. For a folder save,
/// the latest over the folder and every entry directly inside it. `None`
/// when anything cannot be checked. The status-change time is set by the
/// system on every write, rename and replacement and cannot be set back by
/// a program.
fn last_changed(path: &Path) -> Option<i64> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.is_dir() {
        // A folder save: the folder and everything directly inside it.
        let mut latest = folder_last_changed(path)?;
        for entry in std::fs::read_dir(path).ok()? {
            latest = latest.max(stamp(&std::fs::symlink_metadata(entry.ok()?.path()).ok()?));
        }
        return Some(latest);
    }
    Some(stamp(&meta))
}

fn stamp(meta: &std::fs::Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt;
    let up = |seconds: i64, nanos: i64| seconds.saturating_add(i64::from(nanos > 0));
    up(meta.mtime(), meta.mtime_nsec()).max(up(meta.ctime(), meta.ctime_nsec()))
}

/// When a folder itself last changed (an entry made, removed or renamed).
fn folder_last_changed(path: &Path) -> Option<i64> {
    Some(stamp(&std::fs::symlink_metadata(path).ok()?))
}

fn clock_ms(clock: libc::clockid_t) -> Option<i64> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime only fills the struct it is given.
    if unsafe { libc::clock_gettime(clock, &mut time) } != 0 {
        return None;
    }
    Some(time.tv_sec.saturating_mul(1000) + time.tv_nsec / 1_000_000)
}

/// How far the wall clock is ahead of the time-since-boot clock, in
/// milliseconds: in effect when the machine booted, as the wall clock
/// stands now. It changes only when the wall clock is set or stepped.
///
/// A launcher notes this **before** a launch can start anything and hands
/// it to [`LaunchTracker::process_started`], which works out the launch's
/// start in wall-clock time from it and refuses to prove anything if the
/// wall clock has been stepped since (file times and the start time would
/// then be on different clocks).
pub fn clock_offset_ms() -> Option<i64> {
    Some(clock_ms(libc::CLOCK_REALTIME)? - clock_ms(libc::CLOCK_BOOTTIME)?)
}

/// The wall clock may drift this far against the time-since-boot clock
/// between the note before the launch and the judging (ordinary gradual
/// correction); more is a step, and nothing is then proven.
const CLOCK_STEP_MS: i64 = 1_000;

/// The wall-clock second the launch's first process started in, or `None`
/// when that cannot be stated safely. `clock_before` is
/// [`clock_offset_ms`] as noted before the launch, `clock_now` the same
/// now, `now_unix` the wall clock now.
pub fn launch_start_unix(
    start_ticks: Option<u64>,
    clock_before: Option<i64>,
    clock_now: Option<i64>,
    now_unix: i64,
) -> Option<i64> {
    let (ticks, before, now) = (start_ticks?, clock_before?, clock_now?);
    if (before - now).abs() > CLOCK_STEP_MS {
        return None;
    }
    // SAFETY: sysconf only reads a system constant.
    let per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if per_second <= 0 {
        return None;
    }
    let since_boot_ms = (ticks as i64).saturating_mul(1000) / per_second as i64;
    let start = before.saturating_add(since_boot_ms).div_euclid(1000);
    // A start in the future is no start time at all.
    (start <= now_unix).then_some(start)
}

/// The folder a save sits in.
fn folder_of(save: &DiscoveredSave) -> Option<PathBuf> {
    save.source_path.parent().map(Path::to_path_buf)
}

/// What the saves of one emulator held, read before that emulator was
/// started (see [`LaunchTracker::baseline_before_start`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Baseline {
    pub launch: SessionLaunch,
    pub marks: Vec<SaveMark>,
    /// Save files, folders or cards that could not be read.
    pub unread: Vec<PathBuf>,
    /// The folders searched, where their location is a setting.
    pub places: Vec<PathBuf>,
    /// The disk each folder that was searched, or that held a save, was
    /// on (see [`disks_of`]).
    pub disks: Vec<(PathBuf, u64)>,
    /// The mount each of those folders was on.
    pub mounts: Vec<(PathBuf, u64)>,
    pub captured_unix: i64,
}

/// Whether discovery knows where `emulator` keeps its saves.
pub fn saves_known_for(emulator: &str) -> bool {
    NATIVE_NAMES
        .iter()
        .any(|(_, known)| known.eq_ignore_ascii_case(emulator))
}

/// One fresh read of the saves.
struct Fresh {
    saves: Vec<DiscoveredSave>,
    /// Save files, folders or cards that are there but were not taken in.
    unread: Vec<PathBuf>,
    /// The folders searched, where the location is a setting.
    places: Vec<PathBuf>,
    /// Card files taken whole (a narrowed look only).
    whole: Vec<(PathBuf, String)>,
    /// The disk each of those folders was on.
    disks: Vec<(PathBuf, u64)>,
    /// The mount each of those folders was on.
    mounts: Vec<(PathBuf, u64)>,
    /// Oddly named files among RetroArch's saves that were there but were
    /// not cards, and so were not read (a narrowed look).
    seen: Vec<PathBuf>,
    /// Battery saves that were memory cards at this read (a narrowed look).
    as_cards: Vec<PathBuf>,
}

/// Notes that a battery save was a memory card, looked at save by save, at
/// this read. It is never itself reported as changed (see
/// [`super::session::CARD_NOTE`]).
fn card_note(path: &Path) -> SaveMark {
    note(super::session::CARD_NOTE, path)
}

/// Notes that an oddly named file among RetroArch's saves was there, was
/// not a card, and was not read (see [`super::session::SEEN_NOTE`]).
fn seen_note(path: &Path) -> SaveMark {
    note(super::session::SEEN_NOTE, path)
}

fn note(kind: &str, path: &Path) -> SaveMark {
    SaveMark {
        save_id: format!("{kind}{}", path.display()),
        label: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        content_sha256: String::new(),
        last_changed_unix: None,
        before_launch: None,
        folder: None,
        path: Some(path.to_path_buf()),
    }
}

/// Folders, each with a number noted for it (its disk, or its mount).
type Noted = Vec<(PathBuf, u64)>;

/// The disk (device number) of every folder that was listed, or that held
/// something read. A folder that is not there is noted by the nearest
/// folder above it that is, so that a disk put in below it later shows. A
/// disk taken away during a session leaves the folder it was mounted on
/// behind, empty: the folder is then on another disk, which is how
/// [`same_disks`] tells that from saves really deleted.
///
/// Fails when a folder is on another disk now than when it was listed: a
/// disk came or went while the saves were being read, and what was read is
/// not one state of one place.
fn disks_of(
    searched: &[(PathBuf, Option<(u64, u64)>)],
    places: &[PathBuf],
    saves: &[DiscoveredSave],
    whole: &[(PathBuf, String)],
    unread: &[PathBuf],
) -> Result<(Noted, Noted), ContinuityError> {
    let moved = |folder: &Path| {
        ContinuityError::Io(format!(
            "a disk came or went at {} while the saves were being read",
            folder.display()
        ))
    };
    let mut folders: Vec<PathBuf> = places.to_vec();
    for (folder, listed_on) in searched {
        let now = nearest(folder);
        if now.is_none() {
            // Not a disk that moved: the folder, or what is above it,
            // cannot be looked at (or the system does not say which mount a
            // folder is on).
            return Err(ContinuityError::Io(format!(
                "the disk and mount of {} could not be told",
                folder.display()
            )));
        }
        match (listed_on, now) {
            // There when listed: still there, on the same disk and mount.
            (Some(then), Some((at, now))) if &at == folder && now == *then => {
                folders.push(at);
            }
            (Some(_), _) => return Err(moved(folder)),
            // Not there when listed: the nearest folder above stands for it.
            (None, Some((at, _))) if &at != folder => folders.push(at),
            (None, _) => return Err(moved(folder)),
        }
    }
    let held = saves
        .iter()
        .map(|save| &save.source_path)
        .chain(whole.iter().map(|(path, _)| path))
        .chain(unread);
    for path in held {
        if let Some(parent) = path.parent() {
            folders.push(parent.to_path_buf());
        }
        if std::fs::metadata(path).is_ok_and(|meta| meta.is_dir()) {
            folders.push(path.clone());
        }
    }
    folders.sort();
    folders.dedup();
    let (mut disks, mut mounts) = (Vec::new(), Vec::new());
    for folder in folders {
        match super::discover::disk_and_mount(&folder) {
            Ok((disk, mount)) => {
                disks.push((folder.clone(), disk));
                mounts.push((folder, mount));
            }
            Err(error) => {
                return Err(ContinuityError::Io(format!(
                    "the save folder {} could not be looked at: {error}",
                    folder.display()
                )));
            }
        }
    }
    Ok((disks, mounts))
}

/// `folder`, or the nearest folder above it that is there, with its disk
/// and its mount. `None` when that cannot be told (a folder that cannot be
/// looked at).
fn nearest(folder: &Path) -> Option<(PathBuf, (u64, u64))> {
    let mut at = Some(folder);
    while let Some(path) = at {
        match super::discover::disk_and_mount(path) {
            Ok(found) => return Some((path.to_path_buf(), found)),
            // Not there, or under something that is a file: look above.
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                at = path.parent()
            }
            Err(_) => return None,
        }
    }
    None
}

/// The disk of the nearest folder above `folder` (or `folder` itself) that
/// the read before the session noted.
pub(crate) fn noted_disk(before: &[(PathBuf, u64)], folder: &Path) -> Option<u64> {
    before
        .iter()
        .filter(|(known, _)| folder.starts_with(known))
        .max_by_key(|(known, _)| known.components().count())
        .map(|(_, disk)| *disk)
}

/// Whether the folders of the read before a session are still on the disks
/// they were on, and every folder of the read after it is on the disk of
/// the nearest folder above it that the first read noted. A folder that has
/// gone is judged by the nearest one above it that is still there: on
/// another disk, its disk went. (A folder that was listed and has gone
/// altogether also leaves the second read with a folder the first did not
/// note, the one above it; that too is not compared.) When this is false
/// a disk came or went, or the saves are somewhere the first read never
/// looked, and what is there now is not the same place: nothing is compared.
pub(crate) fn same_disks(before: &[(PathBuf, u64)], after: &[(PathBuf, u64)]) -> bool {
    same_noted(before, after, |folder| {
        nearest(folder).map(|(_, (disk, _))| disk)
    })
}

/// The same for mounts: whether every noted folder is still on the mount it
/// was on, and every folder of the second read on the mount of the nearest
/// noted folder above it. This is what shows a folder mounted from
/// elsewhere on the same disk being taken away, or put in, or taken away
/// and put back (which is another mount: what was done to the saves
/// meanwhile is not known).
pub(crate) fn same_mounts(before: &[(PathBuf, u64)], after: &[(PathBuf, u64)]) -> bool {
    same_noted(before, after, |folder| {
        nearest(folder).map(|(_, (_, mount))| mount)
    })
}

fn same_noted(
    before: &[(PathBuf, u64)],
    after: &[(PathBuf, u64)],
    now: impl Fn(&Path) -> Option<u64>,
) -> bool {
    let unmoved = before
        .iter()
        .all(|(folder, noted)| now(folder) == Some(*noted));
    let new_on_the_same = after
        .iter()
        .all(|(folder, found)| noted_disk(before, folder) == Some(*found));
    unmoved && new_on_the_same
}

/// The mark of a card file taken whole.
fn whole_mark((path, content): &(PathBuf, String)) -> SaveMark {
    SaveMark {
        save_id: format!("whole:{}", path.display()),
        label: format!(
            "{} (the whole card file)",
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        ),
        content_sha256: content.clone(),
        last_changed_unix: None,
        before_launch: None,
        folder: None,
        path: Some(path.clone()),
    }
}

/// Reads every save file's content afresh: never a remembered
/// fingerprint, whatever the file's size and times say. `only` narrows
/// the look to one emulator and profile. Fails, rather than return a
/// list with saves missing from it, when a save folder could not be
/// listed or located. No in-use check: this is only for content marks.
fn fresh_read(
    roots: &DiscoveryRoots,
    only: Option<&SessionLaunch>,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Fresh, ContinuityError> {
    let report = discover(
        roots,
        DiscoveryOptions {
            cancel,
            only: only.map(|launch| (launch.emulator.as_str(), launch.profile.as_str())),
            ..Default::default()
        },
    )?;
    if report.stats.stopped_at_limit {
        return Err(ContinuityError::Io(
            "there are too many save files to read them all".into(),
        ));
    }
    if let Some(first) = report.incomplete.first() {
        return Err(ContinuityError::Io(format!(
            "the saves could not all be found: {first}"
        )));
    }
    let mut unread = report.unread;
    unread.extend(
        report
            .saves
            .iter()
            .filter(|save| save.content_sha256.is_none())
            .map(|save| save.source_path.clone()),
    );
    unread.sort();
    unread.dedup();
    let mut places = report.places;
    places.sort();
    places.dedup();
    let (disks, mounts) = disks_of(
        &report.searched,
        &places,
        &report.saves,
        &report.whole_files,
        &unread,
    )?;
    Ok(Fresh {
        saves: report.saves,
        unread,
        places,
        whole: report.whole_files,
        disks,
        mounts,
        seen: report.seen,
        as_cards: report.as_cards,
    })
}

/// Reads the saves of the emulator `launch` names, and only those, afresh.
/// Needs nothing but the profile to look in: no journal, no session. For a
/// launcher that **holds the start of the emulator until this has
/// returned**. `cancel` stops the read (it then fails).
pub fn read_baseline(
    roots: &DiscoveryRoots,
    launch: &SessionLaunch,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Baseline, ContinuityError> {
    let read = fresh_read(roots, Some(launch), cancel)?;
    // Everything a narrowed look returns was found under the starting
    // emulator's own profile, wherever its settings put the save folder.
    let marks = read
        .saves
        .iter()
        .filter(|save| save.emulator.eq_ignore_ascii_case(&launch.emulator))
        .filter_map(mark_of)
        .chain(read.whole.iter().map(whole_mark))
        .chain(read.as_cards.iter().map(|path| card_note(path)))
        .chain(read.seen.iter().map(|path| seen_note(path)))
        .map(|mut mark| {
            mark.before_launch = Some(true);
            mark
        })
        .collect();
    Ok(Baseline {
        launch: launch.clone(),
        marks,
        unread: read.unread,
        places: read.places,
        disks: read.disks,
        mounts: read.mounts,
        captured_unix: now_unix(),
    })
}

pub struct LaunchTracker {
    roots: DiscoveryRoots,
    journal: SessionJournal,
    device_id: String,
}

impl LaunchTracker {
    /// `root` is the save-continuity folder; `roots` is where saves are
    /// looked for (the person's profile, or a scratch one).
    pub fn open(
        root: &Path,
        roots: DiscoveryRoots,
        device_id: &str,
    ) -> Result<Self, ContinuityError> {
        Ok(Self {
            roots,
            // Sandboxed emulators: the system's own crash records, read-only.
            // Wherever that source is missing the answer is simply "not known".
            journal: SessionJournal::open(root, device_id)?
                .with_crash_source(Box::new(super::crash_evidence::ApportLog::system())),
            device_id: device_id.to_string(),
        })
    }

    pub fn journal(&self) -> &SessionJournal {
        &self.journal
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    fn fresh(&self, only: Option<&SessionLaunch>) -> Result<Fresh, ContinuityError> {
        fresh_read(&self.roots, only, None)
    }

    fn saves(&self) -> Result<Vec<DiscoveredSave>, ContinuityError> {
        Ok(self.fresh(None)?.saves)
    }

    /// Reads the saves of the emulator `launch` names, and only those. For
    /// a launcher that **holds the start of the emulator until this has
    /// returned**: the result is then what the saves held before the
    /// emulator existed, by construction and not by any comparison of times.
    pub fn baseline_before_start(
        &self,
        launch: &SessionLaunch,
    ) -> Result<Baseline, ContinuityError> {
        read_baseline(&self.roots, launch, None)
    }

    /// A launch was asked for by a launcher that holds the emulator's start
    /// for [`Self::baseline_before_start`]. No save is read here.
    pub fn begin_gated(
        &self,
        game: SessionGame,
        expected_emulator: &str,
    ) -> Result<SessionRecord, ContinuityError> {
        self.journal.reconcile(&ProcFs, now_unix());
        let launch = SessionLaunch {
            emulator: expected_emulator.to_string(),
            ..Default::default()
        };
        let record = self.journal.request(game, launch, Vec::new())?;
        // Not tracked until a read made before the start says otherwise: a
        // session that never gets that far must not look like one whose
        // saves were read.
        self.journal.set_save_tracking(
            &record.session_id,
            SaveTracking::Unavailable {
                reason: "the saves were not read before the emulator started".into(),
            },
            Vec::new(),
            Vec::new(),
            NotedPlaces::default(),
        )
    }

    /// The gated launcher started `process`. `baseline` is what its gate
    /// read for this very start, or why it read nothing. A baseline for a
    /// different emulator or profile than the one that started is refused.
    pub fn process_started_gated(
        &self,
        session_id: &str,
        process: &StartedProcess,
        baseline: Result<(Baseline, u64, u64), String>,
        processes: &dyn ProcessTable,
    ) -> Result<SessionRecord, ContinuityError> {
        let launch = classify_started(&process.executable, &process.arguments);
        self.journal.mark_started_as(
            session_id,
            process.pid,
            launch.clone(),
            process.start_ticks,
            processes,
        )?;
        let unavailable = |reason: String| {
            (
                SaveTracking::Unavailable { reason },
                Vec::new(),
                Vec::new(),
                NotedPlaces::default(),
            )
        };
        let (tracking, marks, unread, noted) = match baseline {
            Ok((baseline, held_milliseconds, queued_milliseconds))
                if baseline.launch.emulator == launch.emulator
                    && baseline.launch.profile == launch.profile =>
            {
                (
                    SaveTracking::Established {
                        captured_unix: baseline.captured_unix,
                        held_milliseconds,
                        queued_milliseconds,
                    },
                    baseline.marks,
                    baseline.unread,
                    NotedPlaces {
                        places: baseline.places,
                        disks: baseline.disks,
                        mounts: baseline.mounts,
                    },
                )
            }
            Ok(_) => unavailable("the saves read before the start were another emulator's".into()),
            Err(reason) => unavailable(reason),
        };
        self.journal
            .set_save_tracking(session_id, tracking, marks, unread, noted)
    }

    /// A launch was asked for. Which emulator will really start is not known
    /// yet, so every save's content is marked; the session keeps only the
    /// right emulator's once the process exists.
    pub fn begin(
        &self,
        game: SessionGame,
        expected_emulator: &str,
    ) -> Result<SessionRecord, ContinuityError> {
        // Settle anything a previous run of EmuWiz left open first.
        self.journal.reconcile(&ProcFs, now_unix());
        // The launcher may already have started the emulator by now, so
        // these marks are unproven until `process_started` has compared each
        // file's last change (looked up only after its content was read)
        // with the moment the launch's first process began.
        let before: Vec<SaveMark> = self
            .saves()?
            .iter()
            .filter_map(|save| {
                let mut mark = mark_of(save)?;
                mark.last_changed_unix = last_changed(&save.source_path);
                mark.before_launch = Some(false);
                mark.folder = folder_of(save);
                Some(mark)
            })
            .collect();
        // Looked up after the saves were read, like the files' own times.
        let mut folders: Vec<FolderMark> = Vec::new();
        for folder in before.iter().filter_map(|mark| mark.folder.as_ref()) {
            if folders.iter().all(|known| &known.path != folder) {
                folders.push(FolderMark {
                    path: folder.clone(),
                    last_changed_unix: std::fs::symlink_metadata(folder)
                        .ok()
                        .filter(std::fs::Metadata::is_dir)
                        .and_then(|_| folder_last_changed(folder)),
                    before_launch: false,
                });
            }
        }
        let launch = SessionLaunch {
            emulator: expected_emulator.to_string(),
            ..Default::default()
        };
        let record = self.journal.request(game, launch, before)?;
        self.journal.note_folders(&record.session_id, folders)
    }

    /// The launcher started `process` for this session. `clock_before` is
    /// [`clock_offset_ms`] as the launcher noted it before the launch could
    /// start anything; without it no "before" mark is proven.
    pub fn process_started(
        &self,
        session_id: &str,
        process: &StartedProcess,
        clock_before: Option<i64>,
    ) -> Result<SessionRecord, ContinuityError> {
        self.process_started_at(
            session_id,
            process,
            &ProcFs,
            launch_start_unix(
                process.start_ticks,
                clock_before,
                clock_offset_ms(),
                now_unix(),
            ),
        )
    }

    /// As [`Self::process_started_in`], with the start time of the launch's
    /// first process stated by the caller.
    pub fn process_started_at(
        &self,
        session_id: &str,
        process: &StartedProcess,
        processes: &dyn ProcessTable,
        launch_start_unix: Option<i64>,
    ) -> Result<SessionRecord, ContinuityError> {
        let launch = classify_started(&process.executable, &process.arguments);
        // First, and quickly, note the process itself; only then the slower
        // look at the saves.
        self.journal.mark_started_as(
            session_id,
            process.pid,
            launch.clone(),
            process.start_ticks,
            processes,
        )?;
        self.journal
            .judge_baselines(session_id, launch_start_unix)?;
        let its_saves: Vec<String> = self
            .saves()?
            .iter()
            .filter(|save| {
                save.emulator.eq_ignore_ascii_case(&launch.emulator)
                    && save.profile.eq_ignore_ascii_case(&launch.profile)
            })
            .map(|save| save.id.clone())
            .collect();
        self.journal.keep_saves(session_id, &its_saves)
    }

    /// Picks up what a launcher has started, and notes that the emulator is
    /// still alive. Call a moment into a launch **and then regularly while
    /// it runs**: a crash record can be tied to the emulator only if it was
    /// seen alive within `CRASH_RECORD_WINDOW_SECONDS` (ten minutes) of
    /// being found gone, and that time is stored at most once a minute, so
    /// the interval between calls has to stay well under nine minutes.
    /// Cheap enough for once every few seconds. An error here is about the
    /// record only; the caller should ignore it, never fail a launch on it.
    pub fn look_again(&self, session_id: &str) -> Result<SessionRecord, ContinuityError> {
        self.journal.observe_running(session_id, &ProcFs)
    }

    /// The launch is over. `exit` is the started process's own exit, when a
    /// process was started at all; `failure` is the launcher's explanation
    /// when it could not start one.
    pub fn finished(
        &self,
        session_id: &str,
        exit: Option<ExitSummary>,
        failure: Option<&str>,
    ) -> Result<SessionRecord, ContinuityError> {
        self.finished_in(session_id, exit, failure, &ProcFs)
    }

    pub fn finished_in(
        &self,
        session_id: &str,
        exit: Option<ExitSummary>,
        failure: Option<&str>,
        processes: &dyn ProcessTable,
    ) -> Result<SessionRecord, ContinuityError> {
        let record = self.journal.get(session_id)?;
        if record.state == SessionState::Requested {
            return self.journal.mark_never_started(
                session_id,
                failure.unwrap_or("the launch ended without an emulator process being started"),
            );
        }
        let observed = match exit {
            Some(ExitSummary::Code(code)) => ExitObservation::Code(code),
            Some(ExitSummary::Signal {
                signal,
                core_dumped,
            }) => ExitObservation::Signal {
                signal,
                core_dumped,
            },
            Some(ExitSummary::Unavailable) | None => ExitObservation::Unavailable,
        };
        if matches!(record.save_tracking, Some(SaveTracking::Unavailable { .. })) {
            // No starting content: nothing is compared, nothing is claimed.
            return self
                .journal
                .mark_ended(session_id, observed, None, processes);
        }
        // The session's end is recorded whatever happens to the read of
        // the saves: without a complete second read, or with the saves kept
        // somewhere else than at the first read, nothing is compared.
        // The same kind of look as the first read of this session: one
        // emulator's saves for a gated session, all of them otherwise.
        let gated = record.save_tracking.is_some();
        let read = match self.fresh(gated.then_some(&record.launch)) {
            Ok(read)
                if record
                    .save_places_before
                    .as_ref()
                    .is_none_or(|places| places == &read.places)
                    && record
                        .save_disks_before
                        .as_ref()
                        .is_none_or(|disks| same_disks(disks, &read.disks))
                    && record
                        .save_mounts_before
                        .as_ref()
                        .is_none_or(|mounts| same_mounts(mounts, &read.mounts)) =>
            {
                read
            }
            _ => {
                return self
                    .journal
                    .mark_ended(session_id, observed, None, processes);
            }
        };
        if self
            .journal
            .note_unread_after(session_id, read.unread.clone())
            .is_err()
        {
            return self
                .journal
                .mark_ended(session_id, observed, None, processes);
        }
        let mut after: Vec<SaveMark> = read
            .saves
            .iter()
            .filter(|save| {
                save.emulator.eq_ignore_ascii_case(&record.launch.emulator)
                    && (gated || save.profile.eq_ignore_ascii_case(&record.launch.profile))
            })
            .filter_map(mark_of)
            .chain(read.whole.iter().filter(|_| gated).map(whole_mark))
            .chain(
                read.as_cards
                    .iter()
                    .filter(|_| gated)
                    .map(|path| card_note(path)),
            )
            .chain(
                read.seen
                    .iter()
                    .filter(|_| gated)
                    .map(|path| seen_note(path)),
            )
            .collect();
        if record.folders_before.is_some() {
            let saves = self.saves()?;
            for mark in &mut after {
                mark.folder = saves
                    .iter()
                    .find(|save| save.id == mark.save_id)
                    .and_then(folder_of);
            }
        }
        self.journal
            .mark_ended(session_id, observed, Some(&after), processes)
    }

    /// Settles sessions whose processes are gone. Cheap; safe to call often.
    pub fn settle(&self) -> Vec<SessionRecord> {
        self.journal.reconcile(&ProcFs, now_unix())
    }
}
