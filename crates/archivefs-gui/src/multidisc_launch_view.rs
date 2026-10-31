//! Multi-disc launch decision and guidance shared by Preview, Launch and Game
//! Details. The decision itself is core's [`plan_multidisc_launch`]; this
//! module only supplies the library's media sets and turns the outcome into
//! plain user-facing text. It never claims EmuWiz changes discs in a running
//! emulator: a playlist only lets the emulator offer its own disc controls.
use archivefs_core::launch::multidisc_launch::{
    MultiDiscLaunch, MultiDiscPlaylist, plan_multidisc_launch,
};
use archivefs_core::launch::{LaunchCandidate, LaunchTarget, project_media_set_for_launch};
use archivefs_core::media_set::{MediaFamily, MediaSet, MediaSetState, OrdinalUnit};
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

/// A catalogue-ordered set still counts only while, for every image: the
/// configured source's saved inventory is the claim's catalogue revision, a
/// current, verified identity names the claimed entry, and no other current,
/// content-based identity for that image is ambiguous or names a different
/// entry (filename-only results are ignored). A newer
/// catalogue, a removed source, a re-audit, conflicting evidence or a
/// changed file retires it. Other producers' sets are kept as loaded.
pub(crate) fn dat_order_still_backed(
    set: &MediaSet,
    archive_ids: &std::collections::HashMap<&Path, i64>,
    identities: &DatIdentities,
    inventory_revisions: &std::collections::HashMap<String, (Option<String>, Option<String>)>,
) -> bool {
    use archivefs_core::dat::library_identity_summary::DatProvenanceFreshness;
    use archivefs_core::media_set::dat_disc_order::{
        content_evidence, dat_disc_claim, is_dat_disc_order_set,
    };
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
            let Some((inventory_revision, inventory_sha)) = inventory_revisions.get(&source) else {
                return false;
            };
            if inventory_revision.as_deref() != Some(revision.as_str()) {
                return false;
            }
            let Some(summaries) = archive_ids
                .get(representation.record.source.path.as_path())
                .and_then(|id| identities.get(id))
            else {
                return false;
            };
            let current = summaries
                .iter()
                .filter(|s| {
                    s.provenance_freshness == DatProvenanceFreshness::Current
                        && content_evidence(&s.verification_state)
                })
                .collect::<Vec<_>>();
            let backed = current.iter().any(|summary| {
                summary.is_verified()
                    && summary.source.source_id == source
                    && summary.source.catalogue_sha256.as_deref()
                        == archivefs_core::media_set::dat_disc_order::dat_disc_catalogue_sha256(
                            representation,
                        )
                    && (summary.source.catalogue_sha256.is_none()
                        || &summary.source.catalogue_sha256 == inventory_sha)
                    && summary.source.source_revision.as_deref() == Some(revision.as_str())
                    && summary.canonical.canonical_dat_name.as_deref() == Some(entry.as_str())
            });
            let contested = current.iter().any(|summary| {
                summary.is_ambiguous()
                    || summary
                        .canonical
                        .canonical_dat_name
                        .as_deref()
                        .is_some_and(|name| name != entry)
            });
            backed && !contested
        })
}

/// The library's media sets: saved topology evidence (already checked by
/// [`dat_order_still_backed`] when the library loaded), plus sets derived
/// from the catalogue for every other file. Catalogue-
/// derived sets rest on filename evidence, which the launch composer never
/// treats as a verified order.
pub(crate) fn sets_from(
    saved: &[MediaSet],
    archives: &[archivefs_core::PersistedArchive],
) -> Vec<MediaSet> {
    let mut sets = saved.to_vec();
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
        let sets = Arc::new(sets_from(&snapshot.media_topology, &snapshot.archives));
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
    /// The audited file fingerprints of each CUE disc in a catalogue-ordered
    /// set, from the library snapshot. See [`Self::with_disc_members`].
    pub(crate) disc_members: Arc<BTreeMap<PathBuf, Vec<FileFingerprint>>>,
}

type FileFingerprint = archivefs_core::identity_source::hashing::FileFingerprint;

fn contains(set: &MediaSet, path: &Path) -> bool {
    set.members.iter().any(|member| {
        member
            .representations
            .iter()
            .any(|r| r.record.source.archive_member.is_none() && r.record.source.path == path)
    })
}

/// One numbered medium of a larger set. A bare `(a-b)` range is not enough: in
/// a file name it is usually a date, so such a claim needs trusted evidence.
/// One side of a medium does not show that another medium is missing.
fn lone_member_of_larger_set(set: &MediaSet) -> bool {
    let ([member], Some((expected, _))) = (set.members.as_slice(), &set.expected_count) else {
        return false;
    };
    expected.count > 1
        && member.sides.is_empty()
        && member
            .ordinal
            .as_ref()
            .is_some_and(|ordinal| (1..=expected.count).contains(&ordinal.number))
        && member
            .representations
            .iter()
            .flat_map(|r| &r.record.evidence)
            .any(|evidence| {
                evidence.provenance.trusted()
                    || evidence
                        .ordinal
                        .as_ref()
                        .is_some_and(|ordinal| ordinal.unit != OrdinalUnit::Medium)
            })
}

/// EmuWiz-managed playlists live under the app data directory. An unavailable
/// directory yields an empty (relative) root, which core refuses as unsafe.
pub(crate) fn playlist_root() -> PathBuf {
    archivefs_core::app_dirs::data_path("launch-playlists").unwrap_or_default()
}

impl MultiDiscLaunchContext {
    /// Only the set(s) of several media containing `content` are kept, so
    /// nothing large is copied per frame. A set that expects more media than
    /// the one found still counts, so its missing discs are shown.
    pub(crate) fn for_content(
        all: &[MediaSet],
        content: &Path,
        archives: &[archivefs_core::PersistedArchive],
    ) -> Option<Self> {
        let sets: Vec<MediaSet> = all
            .iter()
            .filter(|set| {
                (set.members.len() > 1 || lone_member_of_larger_set(set)) && contains(set, content)
            })
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
            disc_members: Arc::default(),
        })
    }

    /// Takes the audited fingerprints of this context's CUE discs from the
    /// library snapshot. Without them a catalogue-ordered CUE disc never gets
    /// a playlist.
    pub(crate) fn with_disc_members(
        mut self,
        all: &std::collections::HashMap<PathBuf, Vec<FileFingerprint>>,
    ) -> Self {
        self.disc_members = Arc::new(
            all.iter()
                .filter(|(path, _)| self.sets.iter().any(|set| contains(set, path)))
                .map(|(path, members)| (path.clone(), members.clone()))
                .collect(),
        );
        self
    }

    /// For a catalogue-ordered set, every CUE disc's files must still be the
    /// objects the catalogue verified. Returns them for Launch to recheck.
    fn audited_members(
        &self,
        playlist: &MultiDiscPlaylist,
    ) -> Result<Vec<FileFingerprint>, String> {
        let dat_ordered = self
            .sets
            .iter()
            .find(|set| set.identity.key.value == playlist.set_id)
            .is_some_and(archivefs_core::media_set::dat_disc_order::is_dat_disc_order_set);
        let mut audited = Vec::new();
        if !dat_ordered {
            return Ok(audited);
        }
        for disc in &playlist.discs {
            let is_cue = disc
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("cue"));
            if !is_cue {
                continue;
            }
            let Some(members) = self.disc_members.get(disc) else {
                return Err(format!(
                    "{} has no current whole-disc verification; audit it again",
                    disc.display()
                ));
            };
            if let Some(reason) = members.iter().find_map(|recorded| {
                archivefs_core::dat::library_identity_summary::DiscFileState::of(recorded).reason()
            }) {
                return Err(reason);
            }
            audited.extend(members.iter().cloned());
        }
        Ok(audited)
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
        // A validated CUE/BIN release goes to DuckStation only; RetroArch is
        // never given one, so it must never be shown a playlist either.
        if candidate.content.container == Some(archivefs_core::launch::LaunchContainerKind::CueBin)
            && matches!(candidate.target, LaunchTarget::RetroArchCore { .. })
            && self.sets.iter().any(|set| set.members.len() > 1)
        {
            return MultiDiscLaunch::SingleDisc {
                reason: "EmuWiz launches CUE/BIN discs with DuckStation only, so RetroArch gets no playlist".into(),
            };
        }
        let decision = plan_multidisc_launch(
            &self.sets,
            content,
            &candidate.target,
            retroarch_core_extensions,
            sandboxed,
            &self.playlist_root,
        );
        match decision {
            MultiDiscLaunch::Playlist(mut playlist) => {
                if let Some(reason) = self.changed_since_scan(&playlist) {
                    return MultiDiscLaunch::SingleDisc { reason };
                }
                match self.audited_members(&playlist) {
                    Ok(audited) => {
                        playlist.audited_members = audited;
                        MultiDiscLaunch::Playlist(playlist)
                    }
                    Err(reason) => MultiDiscLaunch::SingleDisc { reason },
                }
            }
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
    pub(crate) missing_media: Vec<String>,
    pub(crate) conflicts: Vec<String>,
    pub(crate) provenance: Vec<String>,
    pub(crate) selected_launch_disc: Option<String>,
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
    let trusted_ordinals = projection.media_sequence.iter().all(|step| {
        let Some(ordinal) = &step.ordinal else {
            return false;
        };
        set.members
            .iter()
            .find(|member| member.id == step.member_id)
            .is_some_and(|member| {
                member.representations.iter().any(|representation| {
                    representation.record.evidence.iter().any(|evidence| {
                        evidence.provenance.trusted()
                            && evidence
                                .ordinal
                                .as_ref()
                                .is_some_and(|claim| claim.number == ordinal.number)
                    })
                })
            })
    });
    let order_verified = set.state == MediaSetState::CompleteSet
        && set.identity.verified
        && set.confidence == archivefs_core::media_set::MediaSetConfidence::Proven
        && projection.conflicts.is_empty()
        && projection.start_media.is_some()
        && trusted_ordinals
        && duplicate.is_none();
    let unit = set
        .expected_count
        .as_ref()
        .map(|(count, _)| count.unit)
        .or_else(|| {
            set.members
                .iter()
                .find_map(|m| Some(m.ordinal.as_ref()?.unit))
        });
    let noun = match (unit, set.family) {
        (Some(OrdinalUnit::Part), _) => "Part",
        (Some(OrdinalUnit::Reel), _) => "Reel",
        (_, Some(MediaFamily::Floppy)) => "Disk",
        (_, Some(MediaFamily::Tape)) => "Tape",
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
    // An unnumbered member may be the very medium a gap would name, so no
    // number is called missing until every member has one.
    let gaps: &[_] = if set.members.iter().all(|m| m.ordinal.is_some()) {
        &projection.missing_media
    } else {
        &[]
    };
    let total = gaps
        .iter()
        .find_map(|missing| missing.expected_count)
        .map_or(set.members.len(), usize::from);
    let missing_media: Vec<String> = gaps
        .iter()
        .map(|missing| format!("{noun} {} of {total}", missing.number))
        .collect();
    let mut conflicts = projection.conflicts.clone();
    conflicts.extend(set.conflicts.iter().map(|conflict| conflict.detail.clone()));
    if let Some(number) = duplicate {
        conflicts.push(format!(
            "More than one {} claims to be {noun} {number}; the order cannot be trusted",
            noun.to_ascii_lowercase()
        ));
    }
    // A catalogue-ordered set leads with its gap in plain words.
    if let Some(mut gap) = archivefs_core::media_set::dat_disc_order::dat_order_gap(set) {
        gap[..1].make_ascii_uppercase();
        conflicts.insert(0, gap);
    }
    if !order_verified && conflicts.is_empty() && missing_media.is_empty() {
        conflicts.push(projection.explanation.clone());
    }
    conflicts.sort();
    conflicts.dedup();
    let mut problems = Vec::new();
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
    let count = total.max(set.members.len());
    let noun = noun.to_ascii_lowercase();
    let headline = if order_verified {
        format!("{count}-{noun} game · order verified")
    } else {
        format!("{count}-{noun} game · order not verified")
    };
    let disc_label = |path: &Path| {
        discs
            .iter()
            .find(|disc| disc.path.as_deref() == Some(path))
            .map(|disc| disc.label.clone())
    };
    let selected_launch_disc = match decision {
        Some(MultiDiscLaunch::Playlist(playlist)) => playlist
            .discs
            .first()
            .and_then(|path| disc_label(path))
            .map(|label| format!("{label} (playlist start)")),
        Some(MultiDiscLaunch::SingleDisc { .. }) => {
            disc_label(content).map(|label| format!("{label} (launched on its own)"))
        }
        Some(MultiDiscLaunch::Refused { .. }) => None,
        Some(MultiDiscLaunch::NotMultiDisc) => None,
        None => disc_label(content).map(|label| format!("{label} (selected entry)")),
    };
    let mut provenance = projection
        .swap_plan
        .provenance
        .iter()
        .map(|source| {
            let mut label = format!("{:?}: {}", source.kind, source.source);
            if let Some(version) = &source.version {
                label.push_str(&format!(" · {version}"));
            }
            label
        })
        .collect::<Vec<_>>();
    provenance.sort();
    provenance.dedup();
    DiscGuidance {
        headline,
        order_verified,
        discs,
        next_discs,
        missing_media,
        conflicts,
        provenance,
        selected_launch_disc,
        problems,
        launch: decision.and_then(launch_note),
    }
}

/// Game Details section. Read-only; no actions.
pub(crate) fn show_guidance(ui: &mut egui::Ui, guidance: &DiscGuidance) {
    ui.strong(&guidance.headline);
    if let Some(selected) = &guidance.selected_launch_disc {
        ui.label(format!("Selected launch disc: {selected}"));
    }
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
    for missing in &guidance.missing_media {
        ui.colored_label(ui.visuals().warn_fg_color, format!("Missing: {missing}"));
    }
    for conflict in &guidance.conflicts {
        ui.colored_label(ui.visuals().warn_fg_color, format!("Conflict: {conflict}"));
    }
    if !guidance.provenance.is_empty() {
        ui.weak(format!(
            "Order evidence: {}",
            guidance.provenance.join("; ")
        ));
    }
    for problem in &guidance.problems {
        ui.colored_label(ui.visuals().warn_fg_color, problem);
    }
    if let Some(launch) = &guidance.launch {
        ui.label(launch);
    }
}

/// Runs the explicit, bounded re-check of one CUE disc against the configured
/// catalogue its saved verification names, and says what happened. Called on
/// a worker thread from Game Details; never started automatically.
pub(crate) fn run_disc_recheck(
    database_path: &Path,
    dat_sources_config: &Path,
    recheck: &crate::database_load::DiscRecheck,
    cancel: &std::sync::atomic::AtomicBool,
) -> String {
    let catalogues =
        archivefs_core::dat::configured_catalogues::load_configured_catalogues(dat_sources_config);
    run_disc_recheck_with_catalogues(database_path, &catalogues, recheck, cancel)
}

pub(crate) fn run_disc_recheck_with_catalogues(
    database_path: &Path,
    catalogues: &std::collections::HashMap<
        String,
        archivefs_core::dat::configured_catalogues::ConfiguredCatalogue,
    >,
    recheck: &crate::database_load::DiscRecheck,
    cancel: &std::sync::atomic::AtomicBool,
) -> String {
    let Some(source) = catalogues.get(&recheck.source_id) else {
        return "The catalogue that verified this disc is disabled, removed or unavailable, so it cannot be re-checked.".into();
    };
    let catalogue = archivefs_core::CueDiscCatalogue {
        source_id: source.source_id.clone(),
        display_name: source.display_name.clone(),
        dat_path: source.path.clone(),
        kind: source.kind,
    };
    let outcome =
        archivefs_core::Database::open_or_create(database_path).and_then(|mut database| {
            database.revalidate_cue_disc(
                recheck.archive_id,
                &catalogue,
                &archivefs_core::safe_read::TrustedRoots::none(),
                cancel,
            )
        });
    match outcome {
        Ok(archivefs_core::CueDiscRevalidation::Verified { entry }) => {
            format!("Re-checked: every file of the disc matches {entry} again.")
        }
        Ok(archivefs_core::CueDiscRevalidation::NotVerified { reason }) => format!(
            "Re-checked, and the disc is not verified: {reason}. The earlier result was kept for reference."
        ),
        Ok(archivefs_core::CueDiscRevalidation::Refused { reason }) => {
            format!("The disc could not be re-checked: {reason}.")
        }
        Err(error) => format!("The disc could not be re-checked: {error}."),
    }
}

#[cfg(test)]
#[path = "multidisc_launch_view_tests.rs"]
pub(crate) mod tests;
