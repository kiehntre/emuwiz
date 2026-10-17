//! Multi-disc launch decision and guidance shared by Preview, Launch and Game
//! Details. The decision itself is core's [`plan_multidisc_launch`]; this
//! module only supplies the library's media sets and turns the outcome into
//! plain user-facing text. It never claims EmuWiz changes discs in a running
//! emulator: a playlist only lets the emulator offer its own disc controls.
use archivefs_core::launch::multidisc_launch::{
    MultiDiscLaunch, MultiDiscPlaylist, plan_multidisc_launch,
};
use archivefs_core::launch::{LaunchCandidate, project_media_set_for_launch};
use archivefs_core::media_set::{MediaFamily, MediaSet, MediaSetState};
use eframe::egui;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Catalogue size and modification time (seconds) of each disc, as last scanned.
pub(crate) type CatalogueFingerprints = BTreeMap<PathBuf, (Option<u64>, Option<i64>)>;

thread_local! {
    static LIBRARY_SETS: std::cell::RefCell<Option<(u64, Arc<Vec<MediaSet>>)>> =
        const { std::cell::RefCell::new(None) };
}

/// Saved DAT identities per library item, as the snapshot loaded them.
pub(crate) type DatIdentities = std::collections::HashMap<
    i64,
    Vec<archivefs_core::dat::library_identity_summary::LibraryDatIdentitySummary>,
>;

/// A catalogue-ordered set still counts only while every image's own DAT
/// identity is a current, verified match to the very entry the order came
/// from. A newer catalogue revision, a re-audit or a changed file retires it.
fn dat_order_still_backed(
    set: &MediaSet,
    archive_ids: &std::collections::HashMap<&Path, i64>,
    identities: &DatIdentities,
) -> bool {
    use archivefs_core::dat::library_identity_summary::DatProvenanceFreshness;
    use archivefs_core::media_set::dat_disc_order::{dat_disc_claim, is_dat_disc_order_set};
    if !is_dat_disc_order_set(set) {
        return true;
    }
    set.members
        .iter()
        .flat_map(|m| &m.representations)
        .all(|representation| {
            let Some((source, revision, entry)) = dat_disc_claim(representation) else {
                return false;
            };
            let path = &representation.record.source.path;
            archive_ids
                .get(path.as_path())
                .and_then(|id| identities.get(id))
                .is_some_and(|summaries| {
                    summaries.iter().any(|summary| {
                        summary.is_verified()
                            && summary.provenance_freshness == DatProvenanceFreshness::Current
                            && summary.source.source_id == source
                            && summary.source.source_revision.as_deref() == Some(revision.as_str())
                            && summary.canonical.canonical_dat_name.as_deref()
                                == Some(entry.as_str())
                    })
                })
        })
}

/// The library's media sets: saved topology evidence that is still backed,
/// plus sets derived from the catalogue for every other file. Catalogue-
/// derived sets rest on filename evidence, which the launch composer never
/// treats as a verified order.
pub(crate) fn sets_from(
    saved: &[MediaSet],
    archives: &[archivefs_core::PersistedArchive],
    identities: &DatIdentities,
) -> Vec<MediaSet> {
    let archive_ids = archives
        .iter()
        .map(|archive| (archive.absolute_path.as_path(), archive.id))
        .collect();
    let mut sets = saved
        .iter()
        .filter(|set| dat_order_still_backed(set, &archive_ids, identities))
        .cloned()
        .collect::<Vec<_>>();
    let covered = sets
        .iter()
        .flat_map(|set| &set.members)
        .flat_map(|member| &member.representations)
        .map(|r| r.record.source.path.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let records = archives
        .iter()
        .filter(|archive| archive.last_verified_missing_at.is_none())
        .filter(|archive| !covered.contains(&archive.absolute_path))
        .filter_map(crate::media_sets_page::record_from_catalogue)
        .collect();
    sets.extend(
        archivefs_core::media_set::resolve_index(archivefs_core::media_set::index_media(records))
            .sets,
    );
    sets
}

pub(crate) fn library_sets(
    snapshot: &crate::database_load::CachedLibrarySnapshot,
    generation: u64,
) -> Arc<Vec<MediaSet>> {
    LIBRARY_SETS.with(|cache| {
        if let Some((cached, sets)) = cache.borrow().as_ref()
            && *cached == generation
        {
            return sets.clone();
        }
        let sets = Arc::new(sets_from(
            &snapshot.media_topology,
            &snapshot.archives,
            &snapshot.dat_identities,
        ));
        *cache.borrow_mut() = Some((generation, sets.clone()));
        sets
    })
}

/// The verified media sets a launch needs, plus where managed playlists live.
pub(crate) type PlaylistHandoff = (Arc<Vec<MediaSet>>, MultiDiscPlaylist);

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MultiDiscLaunchContext {
    pub(crate) sets: Arc<Vec<MediaSet>>,
    pub(crate) playlist_root: PathBuf,
    /// Catalogue fingerprints of every disc in `sets`. A disc that differs
    /// from its last library scan never goes into a playlist.
    pub(crate) catalogue: Arc<CatalogueFingerprints>,
}

fn contains(set: &MediaSet, path: &Path) -> bool {
    set.members.iter().any(|member| {
        member
            .representations
            .iter()
            .any(|r| r.record.source.archive_member.is_none() && r.record.source.path == path)
    })
}

/// EmuWiz-managed playlists live under the app data directory. An unavailable
/// directory yields an empty (relative) root, which core refuses as unsafe.
pub(crate) fn playlist_root() -> PathBuf {
    archivefs_core::app_dirs::data_path("launch-playlists").unwrap_or_default()
}

impl MultiDiscLaunchContext {
    /// Only the multi-member set(s) containing `content` are kept, so nothing
    /// large is copied per frame.
    pub(crate) fn for_content(
        all: &[MediaSet],
        content: &Path,
        archives: &[archivefs_core::PersistedArchive],
    ) -> Option<Self> {
        let sets: Vec<MediaSet> = all
            .iter()
            .filter(|set| set.members.len() > 1 && contains(set, content))
            .cloned()
            .collect();
        if sets.is_empty() {
            return None;
        }
        let catalogue = archives
            .iter()
            .filter(|archive| sets.iter().any(|set| contains(set, &archive.absolute_path)))
            .map(|archive| {
                (
                    archive.absolute_path.clone(),
                    (archive.size_bytes, archive.modified_time_unix_seconds),
                )
            })
            .collect();
        Some(Self {
            sets: Arc::new(sets),
            playlist_root: playlist_root(),
            catalogue: Arc::new(catalogue),
        })
    }

    pub(crate) fn decide(
        &self,
        candidate: &LaunchCandidate,
        retroarch_core_extensions: Option<&[String]>,
        sandboxed: bool,
    ) -> MultiDiscLaunch {
        let Some(content) = candidate.content.resolved_path.as_deref() else {
            return MultiDiscLaunch::NotMultiDisc;
        };
        let decision = plan_multidisc_launch(
            &self.sets,
            content,
            &candidate.target,
            retroarch_core_extensions,
            sandboxed,
            &self.playlist_root,
        );
        match decision {
            MultiDiscLaunch::Playlist(playlist) => match self.changed_since_scan(&playlist) {
                Some(reason) => MultiDiscLaunch::SingleDisc { reason },
                None => MultiDiscLaunch::Playlist(playlist),
            },
            other => other,
        }
    }

    /// Every disc must match the size and modification time the library scan
    /// recorded; a disc changed since then is not trusted for a playlist.
    fn changed_since_scan(&self, playlist: &MultiDiscPlaylist) -> Option<String> {
        for (disc, identity) in playlist.discs.iter().zip(&playlist.disc_identities) {
            let seconds = identity.modified.and_then(|time| {
                time.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .and_then(|d| i64::try_from(d.as_secs()).ok())
            });
            match self.catalogue.get(disc) {
                Some((Some(size), Some(modified)))
                    if *size == identity.size && Some(*modified) == seconds => {}
                _ => {
                    return Some(format!(
                        "{} differs from the last library scan (rescan the library first)",
                        disc.display()
                    ));
                }
            }
        }
        None
    }

    pub(crate) fn handoff(&self, decision: &MultiDiscLaunch) -> Option<PlaylistHandoff> {
        match decision {
            MultiDiscLaunch::Playlist(playlist) => Some((self.sets.clone(), playlist.clone())),
            _ => None,
        }
    }
}

/// Preview shows exactly the argument Launch will pass.
pub(crate) fn preview_arguments(
    arguments: &mut [OsString],
    content: Option<&Path>,
    decision: &MultiDiscLaunch,
) {
    if let (MultiDiscLaunch::Playlist(playlist), Some(content)) = (decision, content) {
        let _ = archivefs_core::launch::multidisc_launch::substitute_content_argument(
            arguments,
            content,
            &playlist.playlist_path,
        );
    }
}

/// One plain sentence about how discs are handed to the emulator.
pub(crate) fn launch_note(decision: &MultiDiscLaunch) -> Option<String> {
    match decision {
        MultiDiscLaunch::NotMultiDisc => None,
        MultiDiscLaunch::Playlist(playlist) => Some(format!(
            "{} receives an EmuWiz playlist of all {} discs in verified order. Change discs with {}'s own disc controls; EmuWiz does not change discs while you play.",
            playlist.emulator,
            playlist.discs.len(),
            playlist.emulator
        )),
        MultiDiscLaunch::SingleDisc { reason } => Some(format!(
            "Only this disc is launched: {reason}. Change to another disc yourself in the emulator, if it offers that."
        )),
        MultiDiscLaunch::Refused { reason } => Some(format!(
            "Launch is not available: {reason}. EmuWiz will not start a multi-disc game as a single disc here."
        )),
    }
}

/// Per-disc row for Game Details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscRow {
    pub(crate) label: String,
    pub(crate) path: Option<PathBuf>,
    pub(crate) this_entry: bool,
    pub(crate) start: bool,
}

/// Truthful multi-disc guidance for one game entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscGuidance {
    pub(crate) headline: String,
    pub(crate) order_verified: bool,
    pub(crate) discs: Vec<DiscRow>,
    pub(crate) next_discs: Vec<String>,
    pub(crate) problems: Vec<String>,
    pub(crate) launch: Option<String>,
}

/// Built from the existing topology projection; never orders by filename.
pub(crate) fn guidance(
    set: &MediaSet,
    content: &Path,
    decision: Option<&MultiDiscLaunch>,
) -> DiscGuidance {
    let projection = project_media_set_for_launch(set, None);
    // Re-check the stored evidence instead of trusting its saved state: two
    // discs claiming the same number is never a verified order (the launch
    // composer refuses it too).
    let mut numbers = std::collections::BTreeSet::new();
    let duplicate = projection
        .media_sequence
        .iter()
        .filter_map(|step| step.ordinal.as_ref().map(|o| (o.number, step.side.clone())))
        .find(|key| !numbers.insert(key.clone()))
        .map(|(number, _)| number);
    let order_verified = set.state == MediaSetState::CompleteSet
        && projection.conflicts.is_empty()
        && projection.start_media.is_some()
        && duplicate.is_none();
    let noun = match set.family {
        Some(MediaFamily::Floppy) => "Disk",
        Some(MediaFamily::Tape) => "Tape",
        _ => "Disc",
    };
    let mut discs = Vec::new();
    let mut this_index = None;
    for (index, step) in projection.media_sequence.iter().enumerate() {
        let label = match (&step.ordinal, order_verified) {
            (Some(ordinal), true) => format!("{noun} {}", ordinal.number),
            (Some(ordinal), false) => format!("{noun} {} (order not verified)", ordinal.number),
            (None, _) => format!("{noun} with unknown number"),
        };
        let label = match &step.side {
            Some(side) => format!("{label} · Side {}", side.number),
            None => label,
        };
        let path = step
            .preferred_representation
            .as_ref()
            .map(|s| s.path.clone());
        let this_entry = path.as_deref() == Some(content);
        if this_entry {
            this_index = Some(index);
        }
        let start = order_verified
            && projection
                .start_media
                .as_ref()
                .is_some_and(|start| start.member_id == step.member_id);
        discs.push(DiscRow {
            label,
            path,
            this_entry,
            start,
        });
    }
    let next_discs = match (order_verified, this_index) {
        (true, Some(index)) => discs[index + 1..]
            .iter()
            .filter(|disc| disc.path.is_some())
            .map(|disc| disc.label.clone())
            .collect(),
        _ => Vec::new(),
    };
    let mut problems: Vec<String> = projection
        .missing_media
        .iter()
        .map(|missing| missing.detail.clone())
        .collect();
    problems.extend(projection.conflicts.iter().cloned());
    if let Some(number) = duplicate {
        problems.push(format!(
            "More than one {} claims to be {noun} {number}; the order cannot be trusted",
            noun.to_ascii_lowercase()
        ));
    }
    // A catalogue-ordered set leads with its gap in plain words.
    if let Some(mut gap) = archivefs_core::media_set::dat_disc_order::dat_order_gap(set) {
        gap[..1].make_ascii_uppercase();
        problems.insert(0, gap);
    }
    if !order_verified && problems.is_empty() {
        problems.push(projection.explanation.clone());
    }
    for disc in &discs {
        if let Some(path) = &disc.path
            && std::fs::symlink_metadata(path).map_or(true, |m| !m.is_file())
        {
            problems.push(format!(
                "{} is not available at {}",
                disc.label,
                path.display()
            ));
        }
    }
    let count = discs.len();
    let noun = noun.to_ascii_lowercase();
    let headline = if order_verified {
        format!("{count}-{noun} game · order verified")
    } else {
        format!("{count}-{noun} game · order not verified")
    };
    DiscGuidance {
        headline,
        order_verified,
        discs,
        next_discs,
        problems,
        launch: decision.and_then(launch_note),
    }
}

/// Game Details section. Read-only; no actions.
pub(crate) fn show_guidance(ui: &mut egui::Ui, guidance: &DiscGuidance) {
    ui.strong(&guidance.headline);
    for disc in &guidance.discs {
        let mut text = disc.label.clone();
        if disc.start {
            text.push_str(" · starts the game");
        }
        if disc.this_entry {
            text.push_str(" · this entry");
        }
        if disc.this_entry || disc.start {
            ui.label(egui::RichText::new(text).strong());
        } else {
            ui.label(text);
        }
    }
    if !guidance.next_discs.is_empty() {
        ui.label(format!("Next: {}", guidance.next_discs.join(", ")));
    }
    for problem in &guidance.problems {
        ui.colored_label(ui.visuals().warn_fg_color, problem);
    }
    if let Some(launch) = &guidance.launch {
        ui.label(launch);
    }
}

#[cfg(test)]
#[path = "multidisc_launch_view_tests.rs"]
pub(crate) mod tests;
