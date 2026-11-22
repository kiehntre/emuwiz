//! Bulk rename eligibility (owner rule, 2026-11-22): in a bulk run only a
//! verified catalogue name may rename a file. A RomM title stays visible in
//! the preview, labelled as an unverified hint, but its row is Blocked with
//! the reason, and every Apply path refuses an approved row whose live name
//! is not a verified catalogue name (a stale preview included).
//!
//! This is an eligibility rule only: RomM data and game identity are never
//! changed here, and a row that keeps its own file name is not affected.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use archivefs_core::dat::rom_organisation::rename_guard::mode_changes_real_files;
use archivefs_core::dat::rom_organisation::{
    OrganisationPlan, OrganisationPlanEntry, OrganisationStatus,
};

use super::{NameChoice, NameResolver, NameSource};

/// The reason shown on a row whose new name is only a RomM hint.
pub(crate) const UNVERIFIED_HINT_REASON: &str = "Not renamed in bulk: this name is an unverified RomM hint, not a verified catalogue name. Bulk rename uses verified catalogue names only. Check this game against a catalogue first, or rename it on its own.";

/// Whether applying this row would give the file a new name.
fn gives_a_new_name(entry: &OrganisationPlanEntry) -> bool {
    entry.destination_path.file_name() != entry.source_path.file_name()
}

/// A row may rename in bulk only when its name is a verified catalogue name.
/// No recorded choice counts as unverified (fail closed).
fn name_is_verified(choice: Option<&NameChoice>) -> bool {
    matches!(
        choice.map(|choice| &choice.source),
        Some(NameSource::VerifiedDat { .. })
    )
}

/// Preview: every Suggested row that would rename a file from anything other
/// than a verified catalogue name becomes Blocked with
/// [`UNVERIFIED_HINT_REASON`]. Returns how many rows were blocked.
pub(crate) fn block_unverified_names(
    plan: &mut OrganisationPlan,
    choices: &BTreeMap<PathBuf, NameChoice>,
) -> usize {
    if !mode_changes_real_files(plan.mode) {
        return 0;
    }
    let mut blocked = 0;
    for entry in &mut plan.entries {
        if entry.status == OrganisationStatus::Suggested
            && gives_a_new_name(entry)
            && !name_is_verified(choices.get(&entry.source_path))
        {
            entry.status = OrganisationStatus::Blocked;
            entry.reason = Some(UNVERIFIED_HINT_REASON.to_string());
            blocked += 1;
        }
    }
    blocked
}

/// Apply (every path): the name of each approved renaming row is chosen again
/// from live evidence, and the whole Apply is refused unless it is a verified
/// catalogue name. A preview made before this rule, or one whose catalogue
/// check went stale since, is refused here. Nothing is changed either way.
pub(crate) fn refuse_unverified_names(
    plan: &OrganisationPlan,
    approved: &BTreeSet<String>,
    names: &NameResolver,
) -> Result<(), String> {
    refuse_unverified_names_with(plan, approved, &|source| names.choose(source))
}

/// [`refuse_unverified_names`] with the live name choice supplied.
pub(crate) fn refuse_unverified_names_with(
    plan: &OrganisationPlan,
    approved: &BTreeSet<String>,
    live_choice: &dyn Fn(&Path) -> NameChoice,
) -> Result<(), String> {
    if !mode_changes_real_files(plan.mode) {
        return Ok(());
    }
    for entry in plan.suggested() {
        if !approved.contains(entry.source_path.to_string_lossy().as_ref())
            || !gives_a_new_name(entry)
        {
            continue;
        }
        let live = live_choice(&entry.source_path);
        if !name_is_verified(Some(&live)) {
            return Err(format!(
                "{} would be renamed from an unverified name ({}), not a verified catalogue name. Bulk rename uses verified catalogue names only. Nothing was changed. Preview again.",
                entry.source_path.display(),
                live.source_label()
            ));
        }
    }
    Ok(())
}
