//! Bulk naming eligibility (owner rules, 2026-11-22): in a bulk run only a
//! verified catalogue name may give a file or a link a new name, in every
//! organisation mode. In Rename and Move a RomM title stays visible as an
//! unverified hint and its row is Blocked with the reason. In the link modes
//! (linked library, reorganise existing links) such a link keeps the file's
//! own name instead ([`bulk_name`]). Every Apply path refuses an approved row
//! whose live name is not a verified catalogue name (a stale preview included).
//!
//! This is an eligibility rule only: RomM data and game identity are never
//! changed here, and a row that keeps its own file name is not affected.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use archivefs_core::dat::rom_organisation::{
    OrganisationMode, OrganisationPlan, OrganisationPlanEntry, OrganisationStatus,
};

use super::{NameChoice, NameResolver, NameSource};

/// The reason shown on a row whose new name is only a RomM hint.
pub(crate) const UNVERIFIED_HINT_REASON: &str = "Not renamed in bulk: this name is an unverified RomM hint, not a verified catalogue name. Bulk renaming of files and links uses verified catalogue names only. Check this game against a catalogue first, or rename it on its own.";

/// The reason shown on a row whose new name has no verified catalogue source
/// and no RomM title either (for example a name the layout had to adjust).
pub(crate) const UNVERIFIED_NAME_REASON: &str = "Not renamed in bulk: the new name does not come from a verified catalogue name. Bulk renaming of files and links uses verified catalogue names only. Rename it on its own if you want this name.";

/// Link modes: the same two reasons, worded for a link (nothing is renamed
/// there, and single-game Rename would touch the real file instead).
pub(crate) const UNVERIFIED_LINK_HINT_REASON: &str = "Not used for this link: this name is an unverified RomM hint, not a verified catalogue name. Links are named in bulk from verified catalogue names only. Check this game against a catalogue first.";
/// Link modes: the file's own name (the fallback) had to change to be a safe
/// link name, so it is not the original file name either.
pub(crate) const UNSAFE_LINK_FALLBACK_REASON: &str = "No link made in bulk: this file's own name cannot be used as a link name as it is (it had to be changed to be safe), and there is no verified catalogue name to use instead. Rename the file itself first, or check it against a catalogue.";

/// The name a bulk run may give this file or link (owner rules 2026-11-22):
/// a verified catalogue name always. In a link mode any other name (a RomM
/// title, or none) falls back to the file's own name (`None`), so the game
/// still gets its link and no unverified title is used. In Rename and Move
/// the chosen name is kept here and the row is then blocked by
/// [`block_unverified_names`] unless it is verified.
pub(crate) fn bulk_name(mode: OrganisationMode, choice: &NameChoice) -> Option<String> {
    if is_link_mode(mode) && !name_is_verified(Some(choice)) {
        None
    } else {
        choice.name.clone()
    }
}

/// Link modes: where this link's name comes from, in words.
pub(crate) fn link_name_line(choice: Option<&NameChoice>) -> String {
    match choice {
        Some(NameChoice {
            source: NameSource::VerifiedDat { catalogue },
            ..
        }) => format!("Link name: verified catalogue name ({catalogue})"),
        Some(NameChoice {
            source: NameSource::Romm,
            name: Some(title),
            ..
        }) => format!(
            "Link name: original file name (RomM suggests '{title}', not used: not verified)"
        ),
        _ => "Link name: original file name (no verified catalogue name)".to_string(),
    }
}

pub(crate) fn is_link_mode(mode: OrganisationMode) -> bool {
    matches!(
        mode,
        OrganisationMode::OrganiseSymlinkOnly | OrganisationMode::BuildLinkedLibrary
    )
}

/// The reason for `mode`, for a RomM title (`from_romm`) or any other
/// unverified name.
pub(crate) fn unverified_reason(mode: OrganisationMode, from_romm: bool) -> &'static str {
    match (is_link_mode(mode), from_romm) {
        (false, true) => UNVERIFIED_HINT_REASON,
        (false, false) => UNVERIFIED_NAME_REASON,
        (true, true) => UNVERIFIED_LINK_HINT_REASON,
        // A link that keeps its own name only gets a new one when that name
        // had to change to be safe.
        (true, false) => UNSAFE_LINK_FALLBACK_REASON,
    }
}

/// Whether this row was blocked by this rule (any reason).
pub(crate) fn blocked_for_unverified_name(entry: &OrganisationPlanEntry) -> bool {
    unverified_name_label(entry).is_some()
}

/// Whether applying this row would give the file a new name.
fn gives_a_new_name(entry: &OrganisationPlanEntry) -> bool {
    entry.destination_path.file_name() != entry.source_path.file_name()
}

/// The label for the name a row blocked by this rule was not given, or
/// `None` for every other row.
pub(crate) fn unverified_name_label(entry: &OrganisationPlanEntry) -> Option<&'static str> {
    match entry.reason.as_deref() {
        Some(UNVERIFIED_HINT_REASON | UNVERIFIED_LINK_HINT_REASON) => {
            Some("RomM suggests (hint only, not applied)")
        }
        Some(UNVERIFIED_NAME_REASON) => Some("Proposed name (not verified, not applied)"),
        Some(UNSAFE_LINK_FALLBACK_REASON) => Some("Own file name needs changing (no link made)"),
        _ => None,
    }
}

/// A row may rename in bulk only when its name is a verified catalogue name.
/// No recorded choice counts as unverified (fail closed).
fn name_is_verified(choice: Option<&NameChoice>) -> bool {
    matches!(
        choice.map(|choice| &choice.source),
        Some(NameSource::VerifiedDat { .. })
    )
}

/// Preview, every mode: every Suggested row that would give a file or link a
/// new name from anything other than a verified catalogue name becomes
/// Blocked with [`UNVERIFIED_HINT_REASON`] (RomM title) or
/// [`UNVERIFIED_NAME_REASON`]. Returns how many rows were blocked.
pub(crate) fn block_unverified_names(
    plan: &mut OrganisationPlan,
    choices: &BTreeMap<PathBuf, NameChoice>,
) -> usize {
    let mut blocked = 0;
    let plan_mode = plan.mode;
    for entry in &mut plan.entries {
        if entry.status == OrganisationStatus::Suggested
            && gives_a_new_name(entry)
            && !name_is_verified(choices.get(&entry.source_path))
        {
            let choice = choices.get(&entry.source_path);
            let from_romm = matches!(choice.map(|c| &c.source), Some(NameSource::Romm));
            // In a link mode the plan names a link from a RomM title only when
            // it was built outside this rule (a stale preview); otherwise the
            // own file name was used and had to be changed to be safe.
            let names_the_romm_title = from_romm
                && choice.and_then(|c| c.name.as_deref()).is_some_and(|title| {
                    entry
                        .destination_path
                        .file_stem()
                        .is_some_and(|stem| stem.to_string_lossy() == title)
                        || entry
                            .destination_path
                            .file_name()
                            .is_some_and(|name| name.to_string_lossy() == title)
                });
            let reason = if is_link_mode(plan_mode) && !names_the_romm_title {
                UNSAFE_LINK_FALLBACK_REASON
            } else {
                unverified_reason(plan_mode, from_romm)
            };
            entry.status = OrganisationStatus::Blocked;
            entry.reason = Some(reason.to_string());
            blocked += 1;
        }
    }
    blocked
}

/// Every Apply path (Standard Apply and Advanced Protection) runs exactly
/// this before building a transaction: only verified catalogue names may
/// rename (checked first, without reading the platform database), then the
/// existing live revalidation of platform, destination and name.
pub(crate) fn check_live_names(
    plan: &OrganisationPlan,
    approved: &BTreeSet<String>,
    database_path: &Path,
    names: &NameResolver,
) -> Result<(), String> {
    refuse_unverified_names(plan, approved, names)?;
    refuse_changed_names_with(plan, approved, &|source| names.choose(source))?;
    archivefs_core::dat::rom_organisation::revalidate_organisation_plan(
        plan,
        database_path,
        &|source| bulk_name(plan.mode, &names.choose(source)),
    )
}

/// Apply: each approved row's name is derived again from the live name rule
/// ([`bulk_name`]); a row whose name would now differ from the reviewed
/// preview (its catalogue check or name source changed, in either direction)
/// refuses the whole Apply with that reason, before the platform
/// revalidation could report it as a platform change.
pub(crate) fn refuse_changed_names_with(
    plan: &OrganisationPlan,
    approved: &BTreeSet<String>,
    live_choice: &dyn Fn(&Path) -> NameChoice,
) -> Result<(), String> {
    use archivefs_core::dat::rename_plan::{DeriveOutcome, derive_proposed_basename};
    for entry in plan.suggested() {
        if !approved.contains(entry.source_path.to_string_lossy().as_ref()) {
            continue;
        }
        let (Some(own), Some(reviewed)) = (
            entry.source_path.file_name().and_then(|n| n.to_str()),
            entry.destination_path.file_name().and_then(|n| n.to_str()),
        ) else {
            continue; // left to the full revalidation
        };
        let live = bulk_name(plan.mode, &live_choice(&entry.source_path));
        let DeriveOutcome::Ok(now) = derive_proposed_basename(live.as_deref().unwrap_or(own), own)
        else {
            continue; // left to the full revalidation
        };
        if now.proposed_basename != reviewed {
            return Err(format!(
                "The name for {} changed since the preview (its catalogue check or name source changed): the preview named it '{reviewed}', it would now be '{}'. Nothing was changed. Preview again.",
                entry.source_path.display(),
                now.proposed_basename
            ));
        }
    }
    Ok(())
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
    for entry in plan.suggested() {
        if !approved.contains(entry.source_path.to_string_lossy().as_ref())
            || !gives_a_new_name(entry)
        {
            continue;
        }
        let live = live_choice(&entry.source_path);
        if !name_is_verified(Some(&live)) {
            return Err(format!(
                "{} would get a new name from an unverified source ({}), not a verified catalogue name. Bulk renaming of files and links uses verified catalogue names only. Nothing was changed. Preview again.",
                entry.source_path.display(),
                live.source_label()
            ));
        }
    }
    Ok(())
}
