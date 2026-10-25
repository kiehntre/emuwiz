//! The A-Z jump strip's index: which bucket ("#" or a letter) a game's
//! displayed title falls into, where the first game in each bucket sits
//! within the currently visible result set, and which bucket the row
//! currently scrolled to the top belongs to.
//!
//! Deliberately pure and cheap to keep fresh: [`AlphaJumpIndex::refresh`]
//! only re-sorts and re-buckets when the visible index list has actually
//! changed (a search keystroke, a platform pick, or a reloaded library) -
//! never once per frame regardless of how large the library is. It never
//! reads a file, opens a database, or reaches into egui state; it is driven
//! entirely by the caller handing it `visible` and `records` each frame.

use crate::ArchiveRecord;

/// `#` plus `A..=Z`.
pub(crate) const ALPHA_BUCKETS: usize = 27;
/// The bucket reserved for a title that does not start with an ASCII letter
/// (digits, symbols, or anything else) - drawn first, as `#`.
pub(crate) const HASH_BUCKET: usize = 0;

/// The glyph for one bucket, in the strip's own display order: `#` first,
/// then `A` through `Z`.
pub(crate) fn bucket_label(bucket: usize) -> &'static str {
    const LABELS: [&str; ALPHA_BUCKETS] = [
        "#", "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q",
        "R", "S", "T", "U", "V", "W", "X", "Y", "Z",
    ];
    LABELS[bucket]
}

/// Which bucket a displayed title belongs in: `#` for anything not starting
/// with an ASCII letter (leading whitespace is ignored, matching how a
/// person reads a title), `A..=Z` otherwise, case-insensitively.
pub(crate) fn bucket_for_title(title: &str) -> usize {
    match title.trim_start().chars().next() {
        Some(c) if c.is_ascii_alphabetic() => (c.to_ascii_uppercase() as u8 - b'A') as usize + 1,
        _ => HASH_BUCKET,
    }
}

/// The A-Z jump strip's index over the currently visible result set.
///
/// `sorted` is the alphabetical order the browsing rail renders its grid in
/// (by display title) - the same order [`Self::first_position_for`]'s
/// positions index into, so a jump target converts directly to a grid row
/// with no separate lookup. `current_bucket` is a deliberate one-frame-old
/// value: the rail can only learn which grid row scrolled to the top while
/// it draws itself, which happens *after* the strip above it is drawn, so
/// the strip always reads the row the rail reported last frame - the
/// standard immediate-mode pattern, and imperceptible at any real frame
/// rate.
#[derive(Default)]
pub(crate) struct AlphaJumpIndex {
    /// The exact `visible` list this index was last built from. Comparing
    /// against it is what makes a rebuild happen only when the result set
    /// actually changes.
    built_from: Vec<usize>,
    /// `built_from`, reordered alphabetically by display title.
    sorted: Vec<usize>,
    /// Parallel to `sorted`: which bucket each position belongs to.
    bucket_of: Vec<usize>,
    /// Position within `sorted` of the first title in each bucket, or
    /// `None` when the current result set has nothing starting that way.
    first_position: [Option<usize>; ALPHA_BUCKETS],
    /// Which bucket the row at the top of the rail's viewport belonged to,
    /// as of the rail's own last draw - see the type-level doc comment.
    current_bucket: Option<usize>,
}

impl AlphaJumpIndex {
    /// Rebuilds `sorted`/`first_position` from `visible` and `records` only
    /// when `visible` differs from what this index was last built from.
    /// `visible` is already the filtered/platform-narrowed result set
    /// (`GamerLibrarySnapshot::visible`), so this never touches the whole
    /// library - only whatever is currently in play, and only when that
    /// actually changes.
    pub(crate) fn refresh(&mut self, visible: &[usize], records: &[ArchiveRecord]) {
        if self.built_from == visible {
            return;
        }
        self.built_from = visible.to_vec();

        let mut sorted = visible.to_vec();
        sorted.sort_by(|&a, &b| title_key(&records[a]).cmp(&title_key(&records[b])));

        let mut first_position = [None; ALPHA_BUCKETS];
        let mut bucket_of = Vec::with_capacity(sorted.len());
        for (position, &record_index) in sorted.iter().enumerate() {
            let bucket = bucket_for_title(&title_key(&records[record_index]));
            bucket_of.push(bucket);
            first_position[bucket].get_or_insert(position);
        }

        self.sorted = sorted;
        self.bucket_of = bucket_of;
        self.first_position = first_position;
        // The old current-bucket reading no longer corresponds to anything
        // real once the result set has changed; the rail's next draw
        // supplies a fresh one before it is read again.
        self.current_bucket = None;
    }

    /// The visible result set, reordered alphabetically by display title -
    /// what the rail should actually render, so its grid positions and this
    /// index's jump targets agree.
    pub(crate) fn sorted(&self) -> &[usize] {
        &self.sorted
    }

    /// Whether any title in the current result set falls in this bucket -
    /// the letter is enabled only when this is `true`.
    pub(crate) fn is_enabled(&self, bucket: usize) -> bool {
        self.first_position[bucket].is_some()
    }

    /// Position within [`Self::sorted`] of the first title in `bucket`, or
    /// `None` if the current result set has none.
    pub(crate) fn first_position_for(&self, bucket: usize) -> Option<usize> {
        self.first_position[bucket]
    }

    /// The bucket to highlight as "current" in the strip this frame - see
    /// the type-level doc comment about the one-frame lag.
    pub(crate) fn current_bucket(&self) -> Option<usize> {
        self.current_bucket
    }

    /// Called by the rail while it draws: records which bucket the grid row
    /// now at the top of the viewport belongs to, for the strip to read on
    /// the *next* frame.
    pub(crate) fn report_first_visible_position(&mut self, position: usize) {
        self.current_bucket = self.bucket_of.get(position).copied();
    }
}

fn title_key(record: &ArchiveRecord) -> String {
    super::gamer_display_title(record).to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::{Archive, ArchiveHealth, ArchiveMetadata, MountPlan, MountState};
    use std::path::PathBuf;

    fn titled(id: &str, title: &str) -> ArchiveRecord {
        let archive = Archive::from_path(format!("/roms/{id}.sfc")).unwrap();
        let mut record = ArchiveRecord::new(
            MountPlan::new(archive, PathBuf::from("/mnt/archivefs/Test")),
            MountState::NotMountable,
            ArchiveMetadata {
                title: None,
                platform: None,
                region: None,
                languages: None,
                version: None,
                disc: None,
                publisher: None,
                developer: None,
                release_year: None,
                genre: None,
                notes: None,
                source: None,
                synopsis: None,
                players: None,
                rating: None,
            },
            ArchiveHealth::Pending,
        );
        record.metadata.title = Some(title.to_string());
        record
    }

    #[test]
    fn bucket_for_title_is_case_insensitive_and_trims_leading_space() {
        assert_eq!(bucket_for_title("mario"), bucket_for_title("Mario"));
        assert_eq!(bucket_for_title("  Zelda"), bucket_for_title("Zelda"));
    }

    #[test]
    fn digits_and_symbols_land_in_the_hash_bucket() {
        assert_eq!(bucket_for_title("7 Wonders"), HASH_BUCKET);
        assert_eq!(bucket_for_title("...Iru!"), HASH_BUCKET);
        assert_eq!(bucket_for_title(""), HASH_BUCKET);
    }

    #[test]
    fn a_jumps_to_the_first_a_title_and_m_to_the_first_m_title() {
        let records = vec![
            titled("1", "Zelda"),
            titled("2", "Mario"),
            titled("3", "Metroid"),
            titled("4", "Advance Wars"),
        ];
        let visible = vec![0, 1, 2, 3];
        let mut index = AlphaJumpIndex::default();
        index.refresh(&visible, &records);

        let a_bucket = bucket_for_title("Advance Wars");
        let m_bucket = bucket_for_title("Mario");
        let a_position = index
            .first_position_for(a_bucket)
            .expect("an A title exists");
        let m_position = index
            .first_position_for(m_bucket)
            .expect("an M title exists");

        assert_eq!(
            index.sorted()[a_position],
            3,
            "A did not land on Advance Wars"
        );
        // Mario and Metroid are both M; the first in sorted order wins.
        let m_titles: Vec<&str> = ["Mario", "Metroid"].to_vec();
        let landed_title = &title_key(&records[index.sorted()[m_position]]);
        assert!(
            m_titles.iter().any(|t| t.to_lowercase() == *landed_title),
            "M did not land on an M title"
        );
    }

    #[test]
    fn hash_handles_numeric_and_symbol_titles() {
        let records = vec![titled("1", "8-Bit Adventure"), titled("2", "Mario")];
        let visible = vec![0, 1];
        let mut index = AlphaJumpIndex::default();
        index.refresh(&visible, &records);

        let position = index
            .first_position_for(HASH_BUCKET)
            .expect("a numeric title exists");
        assert_eq!(index.sorted()[position], 0);
    }

    #[test]
    fn an_unavailable_letter_is_disabled() {
        let records = vec![titled("1", "Mario"), titled("2", "Metroid")];
        let visible = vec![0, 1];
        let mut index = AlphaJumpIndex::default();
        index.refresh(&visible, &records);

        let q_bucket = bucket_for_title("Quest");
        assert!(!index.is_enabled(q_bucket));
        assert!(index.first_position_for(q_bucket).is_none());
    }

    #[test]
    fn current_bucket_reflects_the_first_visible_position() {
        let records = vec![titled("1", "Advance Wars"), titled("2", "Mario")];
        let visible = vec![0, 1];
        let mut index = AlphaJumpIndex::default();
        index.refresh(&visible, &records);

        assert_eq!(index.current_bucket(), None, "nothing reported yet");
        index.report_first_visible_position(1);
        assert_eq!(index.current_bucket(), Some(bucket_for_title("Mario")));
    }

    #[test]
    fn a_no_op_refresh_does_not_reset_the_reported_current_bucket() {
        let records = vec![titled("1", "Advance Wars"), titled("2", "Mario")];
        let visible = vec![0, 1];
        let mut index = AlphaJumpIndex::default();
        index.refresh(&visible, &records);
        index.report_first_visible_position(1);

        // Refreshing again with the exact same visible set must not discard
        // the frame's reported position - this is the "only rebuild on an
        // actual change" contract that keeps the highlight from flickering.
        index.refresh(&visible, &records);
        assert_eq!(index.current_bucket(), Some(bucket_for_title("Mario")));
    }

    #[test]
    fn a_changed_result_set_forces_a_rebuild() {
        let records = vec![titled("1", "Advance Wars"), titled("2", "Mario")];
        let mut index = AlphaJumpIndex::default();
        index.refresh(&[0, 1], &records);
        index.refresh(&[0], &records);

        assert_eq!(index.sorted(), &[0]);
        let m_bucket = bucket_for_title("Mario");
        assert!(!index.is_enabled(m_bucket), "Mario is no longer visible");
    }
}
