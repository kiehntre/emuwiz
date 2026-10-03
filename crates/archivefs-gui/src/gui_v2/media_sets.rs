//! "Multi-disc games": a read-only review of multi-disc / multi-media
//! releases. Completeness, ordinals, conflicts and swap order come only from
//! the core `media_set` topology engine over the already-loaded catalogue;
//! this file groups and presents them. It never opens files, never writes,
//! and offers navigation only.
use super::{App, Route, Section, backend::Command, library::SharedLibrary};
use crate::media_sets_page::{ordinal_label, record_from_catalogue};
use crate::ui::theme;
use archivefs_core::media_set::MediaFamily;
use archivefs_core::media_set::{
    ConflictKind, MediaSet, MediaSetState, OrdinalUnit, SwapSemantics, TransitionKind, index_media,
    media_swap_plan, resolve_index,
};
use eframe::egui::{self, RichText};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Bucket {
    MissingDisc,
    Conflicting,
    CantTell,
    Ready,
}

impl Bucket {
    pub(super) const ALL: [Bucket; 4] = [
        Bucket::MissingDisc,
        Bucket::Conflicting,
        Bucket::CantTell,
        Bucket::Ready,
    ];
    pub(super) fn title(self) -> &'static str {
        match self {
            Self::MissingDisc => "Missing a disc",
            Self::Conflicting => "Two files claim the same disc, or releases are mixed",
            Self::CantTell => "Can't tell",
            Self::Ready => "Ready",
        }
    }
    pub(super) fn meaning(self) -> &'static str {
        match self {
            Self::MissingDisc => {
                "EmuWiz found some discs of these games but not all of them. It never guesses a missing disc from the title."
            }
            Self::Conflicting => {
                "The evidence contradicts itself, so EmuWiz offers no action here. Review the files involved."
            }
            Self::CantTell => {
                "Nothing in the files confirms these belong together or that the set is complete. Where only file names suggest it, that is said below."
            }
            Self::Ready => "Every expected disc was found, in order, with trustworthy evidence.",
        }
    }
    fn needs_attention(self) -> bool {
        self != Self::Ready
    }
}

#[derive(Clone, Debug)]
pub(super) struct MemberView {
    pub label: String,
    pub paths: Vec<String>,
}

#[derive(Clone, Debug)]
pub(super) struct SetView {
    pub title: String,
    pub platform: String,
    pub media: &'static str,
    pub state: &'static str,
    pub bucket: Bucket,
    pub filename_only: bool,
    pub completeness: String,
    pub present: Vec<String>,
    pub missing: Vec<String>,
    pub members: Vec<MemberView>,
    pub conflicts: Vec<String>,
    pub why: Vec<String>,
    pub swap: Vec<String>,
    /// Catalogue id of the lowest-ordinal member, for "Open game details".
    pub game_id: Option<i64>,
    pub search: String,
}

#[derive(Debug, Default)]
pub(super) struct MediaSetReview {
    pub key: usize,
    pub sets: Vec<SetView>,
    pub single_media: usize,
}

fn unit_name(unit: OrdinalUnit) -> &'static str {
    match unit {
        OrdinalUnit::Disc => "Disc",
        OrdinalUnit::Disk => "Disk",
        OrdinalUnit::Tape => "Tape",
        OrdinalUnit::Medium => "Medium",
        OrdinalUnit::Part => "Part",
        OrdinalUnit::Reel => "Reel",
    }
}

fn state_name(state: MediaSetState) -> &'static str {
    match state {
        MediaSetState::CompleteSet => "Complete set",
        MediaSetState::IncompleteSet => "Incomplete set",
        MediaSetState::AmbiguousSet => "Ambiguous set",
        MediaSetState::ConflictingSet => "Conflicting set",
        MediaSetState::UnverifiedSet => "Unverified set",
        MediaSetState::UnsupportedSet => "Unsupported set",
    }
}

/// Plain-language name for an engine conflict kind. Exhaustive on purpose: a
/// new engine kind must get a friendly label before it can compile.
fn conflict_label(kind: ConflictKind) -> &'static str {
    use ConflictKind::*;
    match kind {
        PlatformConflict => "These files look like they belong to different systems",
        ReleaseConflict => "These files look like different releases of the game",
        VariantConflict => "These files look like different versions or regions",
        OrdinalConflict => "Two files claim to be the same disc number",
        SideConflict => "Two files claim to be the same disk side",
        RoleConflict => "These files seem to have different jobs (for example install vs. play)",
        CountConflict => "The files disagree about how many discs the game has",
        IdentityConflict => "These files appear to be different games",
        CompetingMedia => "More than one file competes for the same disc slot",
        MissingMedium => "A disc appears to be missing",
        MissingSide => "A disk side appears to be missing",
        UnknownCount => "The number of discs could not be worked out",
        UnknownOrdinal => "The disc number of a file could not be worked out",
        UnprovenGrouping => "Nothing confirms these files belong together",
        UnsupportedFormat => "A file is in a format that can't be checked",
        InvalidEvidence => "Some information about a file could not be trusted",
        UnavailableRepresentation => "A file could not be reached",
        UnresolvedRepresentation => "A file could not be matched to a disc",
        RelationshipConflict => "The files' relationships to each other contradict one another",
    }
}

fn is_multi(set: &MediaSet) -> bool {
    set.members.len() > 1
        || set
            .expected_count
            .as_ref()
            .is_some_and(|(c, _)| c.count > 1)
}

fn title_of(set: &MediaSet) -> String {
    if set.identity.key.namespace == "provisional-title" {
        set.identity.key.value.clone()
    } else {
        format!("{}:{}", set.identity.key.namespace, set.identity.key.value)
    }
}

/// Pure projection of one engine set. Never upgrades the engine's judgement.
pub(super) fn view_of(set: &MediaSet, ids: &HashMap<PathBuf, i64>) -> SetView {
    let verified = set.identity.verified;
    let filename_only = !verified && set.identity.provenance.iter().all(|p| !p.trusted());
    let bucket = match set.state {
        MediaSetState::ConflictingSet => Bucket::Conflicting,
        MediaSetState::CompleteSet if verified => Bucket::Ready,
        MediaSetState::IncompleteSet => Bucket::MissingDisc,
        _ => Bucket::CantTell,
    };
    let present: BTreeSet<u16> = set
        .members
        .iter()
        .filter_map(|m| m.ordinal.as_ref().map(|o| o.number))
        .collect();
    let unit = set
        .expected_count
        .as_ref()
        .map(|(c, _)| c.unit)
        .or_else(|| {
            set.members
                .iter()
                .find_map(|m| m.ordinal.as_ref().map(|o| o.unit))
        })
        .unwrap_or(OrdinalUnit::Disc);
    let name = unit_name(unit);
    let missing: Vec<String> = match &set.expected_count {
        Some((count, _)) => (1..=count.count)
            .filter(|n| !present.contains(n))
            .map(|n| format!("{name} {n}"))
            .collect(),
        None => Vec::new(),
    };
    let completeness = match (&set.expected_count, set.state) {
        (_, MediaSetState::ConflictingSet) => {
            "The evidence contradicts itself, so completeness can't be judged.".to_string()
        }
        (Some((count, _)), MediaSetState::CompleteSet) if filename_only => format!(
            "Looks complete from file names only ({} of {} {}s)",
            set.members.len(),
            count.count,
            name.to_lowercase()
        ),
        (Some((count, _)), _) => format!(
            "{} of {} expected {}s found{}",
            set.members.len(),
            count.count,
            name.to_lowercase(),
            if filename_only {
                " (from file names only)"
            } else {
                ""
            }
        ),
        (None, _) => {
            "How many discs there should be is unknown, so completeness can't be confirmed.".into()
        }
    };
    let mut why = vec![format!("Engine state: {}", state_name(set.state))];
    why.push(format!("Confidence: {:?}", set.confidence));
    if let Some((count, provenance)) = &set.expected_count {
        why.push(format!(
            "Expected count {} came from {:?} ({})",
            count.count, provenance.kind, provenance.source
        ));
    }
    for p in &set.identity.provenance {
        why.push(format!(
            "Grouping evidence: {:?} — {} ({})",
            p.kind,
            p.source,
            if p.trusted() {
                "trusted"
            } else {
                "not trusted"
            }
        ));
    }
    why.extend(set.warnings.iter().cloned());
    let mut conflicts: Vec<String> = Vec::new();
    for c in &set.conflicts {
        let text = format!("{:?}: {}", c.kind, c.detail);
        // Only real contradictions are shown up front. "Unproven", unknown-count
        // and missing-medium notes are not contradictions (the engine marks some
        // of them blocking for its own gating); they are already conveyed above
        // and live under Why.
        let contradiction = matches!(
            c.kind,
            ConflictKind::PlatformConflict
                | ConflictKind::ReleaseConflict
                | ConflictKind::VariantConflict
                | ConflictKind::OrdinalConflict
                | ConflictKind::SideConflict
                | ConflictKind::RoleConflict
                | ConflictKind::CountConflict
                | ConflictKind::IdentityConflict
                | ConflictKind::CompetingMedia
                | ConflictKind::RelationshipConflict
        );
        if c.blocking && contradiction {
            conflicts.push(conflict_label(c.kind).to_string());
        }
        // The raw engine vocabulary stays available under Details.
        why.push(text);
    }
    let mut members: Vec<MemberView> = set
        .members
        .iter()
        .map(|m| MemberView {
            label: ordinal_label(m.ordinal.as_ref(), m.sides.iter().next(), set.family),
            paths: m
                .representations
                .iter()
                .map(|r| r.record.source.path.display().to_string())
                .collect(),
        })
        .collect();
    members.sort_by(|a, b| a.label.cmp(&b.label));
    let game_id = set
        .members
        .iter()
        .min_by_key(|m| m.ordinal.as_ref().map(|o| o.number).unwrap_or(u16::MAX))
        .and_then(|m| m.representations.first())
        .and_then(|r| ids.get(&r.record.source.path).copied());
    let swap = swap_lines(set);
    let title = title_of(set);
    let platform = set
        .platform
        .clone()
        .unwrap_or_else(|| "Unknown system".into());
    let mut search = title.to_lowercase();
    for m in &members {
        for p in &m.paths {
            search.push(' ');
            search.push_str(&p.to_lowercase());
        }
    }
    SetView {
        title,
        platform,
        media: match set.family {
            Some(MediaFamily::Optical) => "Optical",
            Some(MediaFamily::Floppy) => "Floppy",
            Some(MediaFamily::Tape) => "Tape",
            None => "Unknown media",
        },
        state: state_name(set.state),
        bucket,
        filename_only,
        completeness,
        present: present.iter().map(|n| format!("{name} {n}")).collect(),
        missing,
        members,
        conflicts,
        why,
        swap,
        game_id,
        search,
    }
}

fn swap_lines(set: &MediaSet) -> Vec<String> {
    let plan = media_swap_plan(set, None);
    let family = plan.semantics.map(|s| match s {
        SwapSemantics::OpticalSequence => MediaFamily::Optical,
        SwapSemantics::FloppySwap => MediaFamily::Floppy,
        SwapSemantics::TapeLoad => MediaFamily::Tape,
    });
    let mut lines = Vec::new();
    for (index, step) in plan.ordered_media.iter().enumerate() {
        let action = if index == 0 {
            "Start with"
        } else {
            match plan.transitions.get(index - 1).map(|t| t.kind) {
                Some(TransitionKind::ChangeSide) => "Then flip to",
                Some(TransitionKind::LoaderToProgram | TransitionKind::ProgramToData) => {
                    "Then load"
                }
                _ => "When prompted, switch to",
            }
        };
        let mut line = format!(
            "{action}: {}",
            ordinal_label(step.ordinal.as_ref(), step.side.as_ref(), family)
        );
        if let Some(path) = &step.preferred_representation {
            line.push_str(&format!(" — {}", path.path.display()));
        }
        lines.push(line);
    }
    for blocker in &plan.blockers {
        lines.push(format!("Review required: {}", blocker.detail));
    }
    lines
}

/// Pure, testable analysis over catalogue rows (what the worker runs).
pub(super) fn analyse_rows(
    rows: &[(PathBuf, i64, archivefs_core::PersistedArchive)],
    key: usize,
) -> MediaSetReview {
    let ids: HashMap<PathBuf, i64> = rows.iter().map(|(p, id, _)| (p.clone(), *id)).collect();
    let records = rows
        .iter()
        .filter(|(_, _, a)| a.last_verified_missing_at.is_none())
        .filter_map(|(_, _, a)| record_from_catalogue(a))
        .collect();
    let report = resolve_index(index_media(records));
    let mut review = MediaSetReview {
        key,
        ..Default::default()
    };
    for set in &report.sets {
        if is_multi(set) {
            review.sets.push(view_of(set, &ids));
        } else {
            review.single_media += 1;
        }
    }
    review.sets.sort_by(|a, b| {
        a.bucket
            .cmp(&b.bucket)
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
    review
}

pub(super) fn analyse(library: &SharedLibrary, key: usize) -> MediaSetReview {
    let rows: Vec<_> = library
        .games
        .iter()
        .map(|g| {
            (
                g.archive.absolute_path.clone(),
                g.archive.id,
                g.archive.clone(),
            )
        })
        .collect();
    analyse_rows(&rows, key)
}

pub(super) fn headline(review: &MediaSetReview) -> String {
    let ready = review
        .sets
        .iter()
        .filter(|s| s.bucket == Bucket::Ready)
        .count();
    let attention = review
        .sets
        .iter()
        .filter(|s| matches!(s.bucket, Bucket::MissingDisc | Bucket::Conflicting))
        .count();
    let unsure = review
        .sets
        .iter()
        .filter(|s| s.bucket == Bucket::CantTell)
        .count();
    format!(
        "{} multi-disc games: {ready} ready · {attention} need attention · {unsure} can't be confirmed",
        review.sets.len()
    )
}

// ---- UI side -------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Chip {
    #[default]
    All,
    Attention,
    Ready,
}

#[derive(Default)]
pub(super) struct MultiDiscState {
    pub review: Option<Arc<MediaSetReview>>,
    pub job: Option<u64>,
    pub chip: Chip,
    pub query: String,
    pub show_all: BTreeSet<Bucket>,
}

const SHOWN_PER_GROUP: usize = 50;

impl App {
    fn media_sets_key(&self) -> usize {
        Arc::as_ptr(&self.library) as usize
    }

    fn start_media_sets_review(&mut self) {
        if self.multi.job.is_some() {
            return;
        }
        let id = self.activity.queue(
            "Looking for multi-disc games",
            Route::Section(Section::MultiDisc),
            false,
        );
        self.multi.job = Some(id);
        let key = self.media_sets_key();
        self.send(
            id,
            Command::MediaSetReview {
                library: self.library.clone(),
                key,
            },
        );
    }

    pub(super) fn media_sets_done(&mut self, review: MediaSetReview) {
        self.multi.job = None;
        // A result for an older library must never replace a newer one.
        if review.key == self.media_sets_key() {
            self.multi.review = Some(Arc::new(review));
        }
    }

    pub(super) fn media_sets_failed(&mut self) {
        self.multi.job = None;
    }

    pub(super) fn multi_disc_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Are my multi-disc games complete?");
        if self.library.games.is_empty() {
            ui.label("There are no games in the catalogue yet, so there is nothing to check.");
            if ui.button("Open Sources").clicked() {
                self.go(Route::Section(Section::Sources));
            }
            return;
        }
        let stale = self
            .multi
            .review
            .as_ref()
            .is_some_and(|r| r.key != self.media_sets_key());
        if (self.multi.review.is_none() || stale) && self.multi.job.is_none() {
            self.start_media_sets_review();
        }
        ui.label("Read from your catalogue; no files were opened or changed. EmuWiz does not swap discs, build playlists or launch games from this page.");
        ui.horizontal_wrapped(|ui| {
            if self.multi.job.is_some() {
                ui.spinner();
                ui.label("Looking for multi-disc games… you can keep browsing.");
            } else if ui.button("Check again").clicked() {
                self.multi.review = None;
            }
        });
        let Some(review) = self.multi.review.clone() else {
            return;
        };
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(RichText::new(headline(&review)).strong());
            ui.small(format!(
                "{} single-disc item(s) are not listed.",
                review.single_media
            ));
        });
        if review.sets.is_empty() {
            ui.label("No multi-disc games found in your catalogue.");
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for (chip, label) in [
                (Chip::All, "All"),
                (Chip::Attention, "Needs attention"),
                (Chip::Ready, "Ready"),
            ] {
                if ui
                    .selectable_label(self.multi.chip == chip, label)
                    .clicked()
                {
                    self.multi.chip = chip;
                }
            }
            ui.label("Search");
            ui.add(
                egui::TextEdit::singleline(&mut self.multi.query)
                    .desired_width(180.0)
                    .hint_text("title or path"),
            );
        });
        let query = self.multi.query.trim().to_lowercase();
        let chip = self.multi.chip;
        let mut go: Option<Route> = None;
        for bucket in Bucket::ALL {
            let visible: Vec<&SetView> = review
                .sets
                .iter()
                .filter(|s| s.bucket == bucket)
                .filter(|s| match chip {
                    Chip::All => true,
                    Chip::Attention => s.bucket.needs_attention(),
                    Chip::Ready => s.bucket == Bucket::Ready,
                })
                .filter(|s| query.is_empty() || s.search.contains(&query))
                .collect();
            if visible.is_empty() {
                continue;
            }
            let all = self.multi.show_all.contains(&bucket);
            egui::CollapsingHeader::new(
                RichText::new(format!("{} ({})", bucket.title(), visible.len())).strong(),
            )
            .id_salt(("multidisc_bucket", bucket as u8))
            .default_open(bucket.needs_attention())
            .show(ui, |ui| {
                ui.label(bucket.meaning());
                let take = if all { visible.len() } else { SHOWN_PER_GROUP };
                for set in visible.iter().take(take) {
                    show_set(ui, set, &mut go);
                }
                if visible.len() > take
                    && ui
                        .button(format!("Show {} more", visible.len() - take))
                        .clicked()
                {
                    self.multi.show_all.insert(bucket);
                }
            });
        }
        if let Some(route) = go {
            self.go(route);
        }
    }
}

fn show_set(ui: &mut egui::Ui, set: &SetView, go: &mut Option<Route>) {
    ui.push_id(&set.title, |ui| {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&set.title).strong());
                ui.small(format!("{} · {} · {}", set.platform, set.media, set.state));
            });
            ui.label(&set.completeness);
            if set.filename_only {
                ui.small("Only the file names suggest these belong together; nothing in the files confirms it.");
            }
            if !set.present.is_empty() {
                ui.small(format!("Found: {}", set.present.join(" · ")));
            }
            if !set.missing.is_empty() {
                ui.colored_label(theme::WARNING, format!("Not found: {}", set.missing.join(" · ")));
            }
            for conflict in &set.conflicts {
                ui.colored_label(theme::DANGER, conflict);
            }
            ui.collapsing(format!("Details — {}", set.title), |ui| {
                for member in &set.members {
                    ui.label(RichText::new(&member.label).strong());
                    for path in &member.paths {
                        ui.small(path);
                    }
                }
                ui.add_space(4.0);
                for line in &set.why {
                    ui.small(line);
                }
            });
            if !set.swap.is_empty() {
                ui.collapsing(format!("Disc swap order — {}", set.title), |ui| {
                    ui.small("Inspection only — EmuWiz does not swap discs or build playlists here.");
                    for line in &set.swap {
                        ui.small(line);
                    }
                });
            }
            ui.horizontal_wrapped(|ui| {
                if let Some(id) = set.game_id
                    && ui.button("Open game details").clicked()
                {
                    *go = Some(Route::Game(id));
                }
                if matches!(set.bucket, Bucket::MissingDisc | Bucket::CantTell)
                    && ui.button("Check my sources").clicked()
                {
                    *go = Some(Route::Section(Section::Sources));
                }
                if set.bucket == Bucket::Conflicting && ui.button("Review in Duplicates").clicked() {
                    *go = Some(Route::Section(Section::Duplicates));
                }
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use ConflictKind::*;

    #[test]
    fn every_conflict_kind_has_a_friendly_primary_label() {
        let all = [
            PlatformConflict,
            ReleaseConflict,
            VariantConflict,
            OrdinalConflict,
            SideConflict,
            RoleConflict,
            CountConflict,
            IdentityConflict,
            CompetingMedia,
            MissingMedium,
            MissingSide,
            UnknownCount,
            UnknownOrdinal,
            UnprovenGrouping,
            UnsupportedFormat,
            InvalidEvidence,
            UnavailableRepresentation,
            UnresolvedRepresentation,
            RelationshipConflict,
        ];
        assert_eq!(all.len(), 19);
        for kind in all {
            let label = conflict_label(kind);
            assert_ne!(label, format!("{kind:?}"));
            assert!(label.contains(' '), "{kind:?} label should be a sentence");
        }
    }
}
