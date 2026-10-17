//! Multi-disc launch decision and guidance shared by Preview, Launch and Game
//! Details. The decision itself is core's [`plan_multidisc_launch`]; this
//! module only supplies the library's media sets and turns the outcome into
//! plain user-facing text. It never claims EmuWiz changes discs in a running
//! emulator: a playlist only lets the emulator offer its own disc controls.
use archivefs_core::launch::multidisc_launch::{
    MultiDiscLaunch, MultiDiscPlaylist, plan_multidisc_launch,
};
use archivefs_core::launch::{LaunchCandidate, project_media_set_for_launch};
use archivefs_core::media_set::{MediaSet, MediaSetState};
use eframe::egui;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The verified media sets a launch needs, plus where managed playlists live.
pub(crate) type PlaylistHandoff = (Arc<Vec<MediaSet>>, MultiDiscPlaylist);

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MultiDiscLaunchContext {
    pub(crate) sets: Arc<Vec<MediaSet>>,
    pub(crate) playlist_root: PathBuf,
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
    pub(crate) fn for_content(all: &[MediaSet], content: &Path) -> Option<Self> {
        let sets: Vec<MediaSet> = all
            .iter()
            .filter(|set| set.members.len() > 1 && contains(set, content))
            .cloned()
            .collect();
        (!sets.is_empty()).then(|| Self {
            sets: Arc::new(sets),
            playlist_root: playlist_root(),
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
        plan_multidisc_launch(
            &self.sets,
            content,
            &candidate.target,
            retroarch_core_extensions,
            sandboxed,
            &self.playlist_root,
        )
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
    let mut discs = Vec::new();
    let mut this_index = None;
    for (index, step) in projection.media_sequence.iter().enumerate() {
        let label = match (&step.ordinal, order_verified) {
            (Some(ordinal), true) => format!("Disc {}", ordinal.number),
            (Some(ordinal), false) => format!("Disc {} (order not verified)", ordinal.number),
            (None, _) => "Disc with unknown number".to_string(),
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
            "More than one disc claims to be Disc {number}; the disc order cannot be trusted"
        ));
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
    let headline = if order_verified {
        format!("{count}-disc game · order verified")
    } else {
        format!("{count}-disc game · order not verified")
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
