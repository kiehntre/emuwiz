//! Read-only local-file-to-catalogue matching for the Playing Library
//! planner.
//!
//! [`build_playing_library_plan`](super::build_playing_library_plan) only
//! ever trusts a caller-supplied [`super::DatArchiveMatch`]; it never
//! verifies anything itself. This module is one honest way to produce those
//! matches: hash each candidate file's whole-file SHA-1 (via the existing,
//! already-policy-gated [`crate::identity_source::hashing::hash_file`]) and
//! look it up in the supplied catalogue's own [`DatIndex`]. Nothing here
//! reads archive members (ZIP/7z contents) - only whole loose files - and
//! nothing here guesses: a hash with no match, or a hash that resolves to
//! more than one *distinct* game, is left unmatched rather than picked.
//!
//! # Multi-file releases fail closed as a whole
//!
//! A CUE sheet's `.bin`/audio tracks and a GDI descriptor's tracks each
//! verify against their own `<rom>` entry, but the `.cue`/`.gdi` file
//! itself is never individually hashed by any real DAT provider - so a
//! plain per-file pass alone would either drop the whole release (the
//! launcher never matches anything) or - worse - treat each of its
//! several verified track files as an independent, competing election for
//! the same game, which [`super::elect_family`] would then wrongly try to
//! rank against each other. [`match_loose_files_against_dat`] resolves
//! this in two passes: first the ordinary whole-file hash match described
//! above; then, for every `.cue`/`.gdi` candidate, its referenced files
//! (via the existing
//! [`crate::ingestion::cue_bin::resolve_cue_all_files_lenient`]/
//! [`crate::ingestion::gdi::resolve_gdi_all_tracks_lenient`] - no second
//! CUE/GDI parser exists here) are checked against the first pass's own
//! matches; if they all resolve safely and verify against exactly one
//! distinct game, the launcher becomes one combined [`DatArchiveMatch`]
//! with those files as [`DatArchiveMatch::companion_paths`].
//!
//! The moment a `.cue`/`.gdi`/`.m3u` launcher *structurally references* a
//! file - names it in a `FILE`/track/playlist line, regardless of whether
//! that reference ends up resolving - every safely-identified file in that
//! reference set is treated as belonging to that one logical release, not
//! as an independent candidate. If the launcher combines successfully,
//! those files are removed from the plain single-file match list so they
//! never also become their own election. If it does not - a missing,
//! unsafe, escaping, or cross-game-ambiguous reference - the *entire*
//! release is refused: the launcher is reported in
//! [`MatchOutcome::rejected_launchers`] with a plain-language reason, and
//! every file it structurally, safely referenced is excluded from the
//! plain single-file pass too, exactly like the launcher itself. A file
//! that happens to sit in the same directory, or share a basename
//! convention, but was never actually named by the descriptor is
//! untouched - only structurally referenced members of the rejected set
//! are suppressed.
//!
//! # M3U multi-disc releases
//!
//! Unlike a CUE/GDI track, a real multi-disc DAT does not encode "Disc 1"/
//! "Disc 2" as one `<game>` with several `<rom>` children: each disc is
//! its own separate `<game>` entry, related only by a strict
//! `"(Disc N of M)"` name token - the same one
//! [`crate::dat::classification::multidisc_group_key`] already recognises
//! for the organiser, reused here unchanged (no second multi-disc
//! detector). An `.m3u` playlist's own referenced lines are read with the
//! existing [`crate::platform_evidence_fusion::cue_m3u_parsing::parse_m3u_references`]
//! (no second M3U parser). Each referenced disc - itself possibly a CUE
//! with its own companions, resolved one level deep via
//! [`crate::ingestion::cue_bin::resolve_cue_all_files_lenient`] - must
//! verify against its own distinct DAT entry, and every one of those
//! entries must share the same multidisc base title; anything else is
//! refused as a whole, exactly as above - the M3U, every disc it
//! structurally names, and each of those discs' own structurally
//! resolved companions are all excluded from independent election.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::dat::classification::multidisc_group_key;
use crate::dat::index::DatIndex;
use crate::dat::model::ParsedDat;
use crate::identity_source::hashing::hash_file;
use crate::ingestion::cue_bin::resolve_cue_all_files_lenient;
use crate::ingestion::gdi::resolve_gdi_all_tracks_lenient;
use crate::platform_evidence_fusion::cue_m3u_parsing::{MAX_PARSE_BYTES, parse_m3u_references};
use crate::safe_read::TrustedRoots;

use super::{DatArchiveMatch, RejectedLauncher};

/// The outcome of trying to combine one launcher's structurally referenced
/// files into a single release. `known_files` always lists every file the
/// launcher safely, structurally referenced - populated regardless of
/// whether `outcome` succeeds, fails, or finds nothing to combine - so a
/// caller that rejects the release can exclude every member of it from
/// independent election, not just the ones already known to verify.
struct LauncherAttempt {
    outcome: Result<Option<DatArchiveMatch>, String>,
    known_files: Vec<PathBuf>,
}

/// The result of one matching run: verified matches (single- and
/// multi-file alike) plus every CUE/GDI launcher found but not safely
/// combinable into one - see the module doc comment.
#[derive(Debug, Clone, Default)]
pub struct MatchOutcome {
    pub matches: Vec<DatArchiveMatch>,
    pub rejected_launchers: Vec<RejectedLauncher>,
}

/// Hashes every candidate and keeps only the ones whose SHA-1 resolves to
/// exactly one game in `dat`, then combines any CUE/GDI launcher's
/// referenced files into one multi-file match - see the module doc
/// comment. Unreadable files, hash refusals, and hash-collisions across
/// more than one distinct game are silently dropped - never guessed.
pub fn match_loose_files_against_dat(
    dat: &ParsedDat,
    candidates: &[PathBuf],
    trusted: &TrustedRoots,
    cancel: &AtomicBool,
) -> MatchOutcome {
    let index = DatIndex::build(dat);

    // Pass 1: ordinary whole-file hash match, exactly as before this
    // module gained multi-file support.
    let mut file_game: BTreeMap<PathBuf, usize> = BTreeMap::new();
    for candidate in candidates {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(hashes) = hash_file(candidate, trusted, Some(cancel)) else {
            continue;
        };
        let refs = index.lookup_sha1(&hashes.sha1);
        let distinct_games: BTreeSet<usize> = refs.iter().map(|entry| entry.game_index).collect();
        if distinct_games.len() == 1 {
            file_game.insert(
                candidate.clone(),
                *distinct_games.iter().next().expect("len() == 1"),
            );
        }
    }

    // Pass 2: M3U launchers, resolved *before* CUE/GDI so that a disc an
    // M3U claims - whether it successfully combines or is rejected - is
    // excluded before pass 3 gets a chance to treat that same CUE/GDI file
    // as its own independent single-disc launcher.
    let mut excluded: BTreeSet<PathBuf> = BTreeSet::new();
    let mut matches = Vec::new();
    let mut rejected_launchers = Vec::new();
    for candidate in candidates {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(extension) = candidate
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
        else {
            continue;
        };
        if extension != "m3u" && extension != "m3u8" {
            continue;
        }
        let attempt = combine_m3u_launcher(candidate, dat, &file_game, trusted);
        record_attempt(
            candidate,
            attempt,
            &mut excluded,
            &mut matches,
            &mut rejected_launchers,
        );
    }

    // Pass 3: CUE/GDI launchers not already excluded by an M3U above. A
    // launcher only ever combines files pass 1 already verified - this
    // pass never hashes anything itself, and never trusts a referenced
    // file it cannot find in `file_game`.
    for candidate in candidates {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if excluded.contains(candidate) {
            continue;
        }
        let Some(extension) = candidate
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
        else {
            continue;
        };
        let (kind, outcomes) = match extension.as_str() {
            "cue" => match resolve_cue_all_files_lenient(candidate) {
                Ok(list) => (
                    "CUE",
                    list.into_iter()
                        .map(|outcome| outcome.map_err(|error| error.to_string()))
                        .collect::<Vec<_>>(),
                ),
                // A CUE extension with no readable/parseable descriptor at
                // all is not this module's concern (e.g. discovery already
                // rejected it, or it belongs to a different platform
                // pass) - only a launcher this module could actually parse
                // but not safely combine is worth reporting.
                Err(_) => continue,
            },
            "gdi" => match resolve_gdi_all_tracks_lenient(candidate) {
                Ok(list) => (
                    "GDI",
                    list.into_iter()
                        .map(|outcome| outcome.map_err(|error| error.to_string()))
                        .collect::<Vec<_>>(),
                ),
                Err(_) => continue,
            },
            _ => continue,
        };
        let attempt = combine_launcher(kind, candidate, &outcomes, &file_game, trusted);
        record_attempt(
            candidate,
            attempt,
            &mut excluded,
            &mut matches,
            &mut rejected_launchers,
        );
    }

    // Pass 4: every pass-1 match not excluded as a companion of an
    // accepted or rejected multi-file release becomes its own ordinary
    // single-file match - unchanged behaviour for CHD, ISO, RVZ,
    // cartridge, and archive releases.
    for (path, game_index) in file_game {
        if excluded.contains(&path) {
            continue;
        }
        matches.push(DatArchiveMatch {
            archive_path: path,
            dat_entry_index: game_index,
            companion_paths: Vec::new(),
        });
    }

    MatchOutcome {
        matches,
        rejected_launchers,
    }
}

/// Applies one [`LauncherAttempt`] to the running match/exclusion state.
/// An accepted combination excludes its own companions plus the launcher;
/// a rejected one excludes every safely-identified file it structurally
/// referenced plus the launcher, so the whole broken release disappears
/// from independent election together, not file by file.
fn record_attempt(
    candidate: &Path,
    attempt: LauncherAttempt,
    excluded: &mut BTreeSet<PathBuf>,
    matches: &mut Vec<DatArchiveMatch>,
    rejected_launchers: &mut Vec<RejectedLauncher>,
) {
    match attempt.outcome {
        Ok(Some(combined)) => {
            for path in &attempt.known_files {
                excluded.insert(path.clone());
            }
            excluded.insert(candidate.to_path_buf());
            matches.push(combined);
        }
        Ok(None) => {
            // Nothing in the reference set verified against the catalogue
            // at all - an ordinary unmatched launcher, silently dropped
            // exactly like an ordinary unmatched single file. Nothing to
            // exclude: an unmatched file was never a Pass 4 candidate.
        }
        Err(reason) => {
            for path in &attempt.known_files {
                excluded.insert(path.clone());
            }
            excluded.insert(candidate.to_path_buf());
            rejected_launchers.push(RejectedLauncher {
                launcher_path: candidate.to_path_buf(),
                reason,
            });
        }
    }
}

/// Tries to combine one CUE/GDI launcher's structurally referenced files
/// (`outcomes`, in declaration order, one lenient per-reference resolution
/// result each) into one [`DatArchiveMatch`]. Any missing, unsafe, or
/// trust-escaping reference fails the *entire* release closed rather than
/// silently dropping just that one file - see the module doc comment.
/// `Ok(None)` means every safely-resolved reference existed but none
/// matched anything (an ordinary unmatched launcher); `Err` names a plain
/// reason for [`RejectedLauncher`].
fn combine_launcher(
    kind: &str,
    launcher: &Path,
    outcomes: &[Result<PathBuf, String>],
    file_game: &BTreeMap<PathBuf, usize>,
    trusted: &TrustedRoots,
) -> LauncherAttempt {
    let mut known_files = Vec::new();
    let mut failures = 0usize;
    for outcome in outcomes {
        match outcome {
            Ok(path) if trusted.contains_canonical(path) => {
                if !known_files.contains(path) {
                    known_files.push(path.clone());
                }
            }
            Ok(_) | Err(_) => failures += 1,
        }
    }
    if failures > 0 {
        return LauncherAttempt {
            outcome: Err(format!(
                "{kind} release is incomplete: {failures} of {} referenced file(s) are missing, unsafe, or escape the trusted source root",
                outcomes.len()
            )),
            known_files,
        };
    }
    let matched_games: BTreeSet<usize> = known_files
        .iter()
        .filter_map(|path| file_game.get(path).copied())
        .collect();
    let outcome = match matched_games.len() {
        0 => Ok(None),
        1 => {
            let dat_entry_index = *matched_games.iter().next().expect("len() == 1");
            Ok(Some(DatArchiveMatch {
                archive_path: launcher.to_path_buf(),
                dat_entry_index,
                companion_paths: known_files.clone(),
            }))
        }
        _ => Err(format!(
            "referenced files verify against {} different catalogue entries, not one",
            matched_games.len()
        )),
    };
    LauncherAttempt {
        outcome,
        known_files,
    }
}

/// Tries to combine one `.m3u` playlist's referenced discs into one
/// multi-disc [`DatArchiveMatch`] - see the module doc comment's "M3U
/// multi-disc releases" section. `known_files` accumulates every disc
/// launcher and disc-companion file structurally, safely identified along
/// the way *regardless* of whether the overall attempt succeeds, so a
/// rejection can suppress the whole set - not just the M3U itself.
/// `Ok(None)` means the playlist is empty or none of its discs matched
/// anything (an ordinary unmatched playlist, silently ignored); `Err`
/// names a plain reason for [`RejectedLauncher`].
fn combine_m3u_launcher(
    launcher: &Path,
    dat: &ParsedDat,
    file_game: &BTreeMap<PathBuf, usize>,
    trusted: &TrustedRoots,
) -> LauncherAttempt {
    let mut known_files = Vec::new();
    let metadata = match std::fs::metadata(launcher) {
        Ok(metadata) => metadata,
        Err(error) => {
            return LauncherAttempt {
                outcome: Err(format!("could not read M3U: {error}")),
                known_files,
            };
        }
    };
    if metadata.len() as usize > MAX_PARSE_BYTES {
        return LauncherAttempt {
            outcome: Err(format!(
                "M3U exceeds the {MAX_PARSE_BYTES}-byte bound for a playlist this module will parse"
            )),
            known_files,
        };
    }
    let contents = match std::fs::read_to_string(launcher) {
        Ok(contents) => contents,
        Err(error) => {
            return LauncherAttempt {
                outcome: Err(format!("could not read M3U: {error}")),
                known_files,
            };
        }
    };
    let references = parse_m3u_references(launcher, &contents);
    if references.is_empty() {
        return LauncherAttempt {
            outcome: Ok(None),
            known_files,
        };
    }

    let mut disc_game_indexes = Vec::new();
    for reference in &references {
        let Some(resolved) = &reference.resolved else {
            return LauncherAttempt {
                outcome: Err(format!(
                    "referenced disc \"{}\" is unsafe: {:?}",
                    reference.raw, reference.rejection
                )),
                known_files,
            };
        };
        let canonical = match std::fs::canonicalize(resolved) {
            Ok(canonical) => canonical,
            Err(_) => {
                return LauncherAttempt {
                    outcome: Err(format!("referenced disc {} is missing", resolved.display())),
                    known_files,
                };
            }
        };
        if !canonical.is_file() || !trusted.contains_canonical(&canonical) {
            return LauncherAttempt {
                outcome: Err(format!(
                    "referenced disc {} lies outside the trusted source folder",
                    canonical.display()
                )),
                known_files,
            };
        }
        known_files.push(canonical.clone());
        let disc_extension = canonical
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase);
        let disc_game_index = if disc_extension.as_deref() == Some("cue") {
            let per_reference = match resolve_cue_all_files_lenient(&canonical) {
                Ok(list) => list,
                Err(error) => {
                    return LauncherAttempt {
                        outcome: Err(format!("disc {} is unusable: {error}", canonical.display())),
                        known_files,
                    };
                }
            };
            let mut disc_files = Vec::new();
            let mut disc_failures = 0usize;
            for outcome in &per_reference {
                match outcome {
                    Ok(path) if trusted.contains_canonical(path) => {
                        if !disc_files.contains(path) {
                            disc_files.push(path.clone());
                        }
                    }
                    Ok(_) | Err(_) => disc_failures += 1,
                }
            }
            known_files.extend(disc_files.iter().cloned());
            if disc_failures > 0 {
                return LauncherAttempt {
                    outcome: Err(format!(
                        "disc {} is missing one or more required companion files",
                        canonical.display()
                    )),
                    known_files,
                };
            }
            let matched: BTreeSet<usize> = disc_files
                .iter()
                .filter_map(|path| file_game.get(path).copied())
                .collect();
            if matched.len() != 1 {
                return LauncherAttempt {
                    outcome: Err(format!(
                        "disc {} does not verify against exactly one catalogue entry",
                        canonical.display()
                    )),
                    known_files,
                };
            }
            *matched.iter().next().expect("len() == 1")
        } else {
            let Some(&game_index) = file_game.get(&canonical) else {
                return LauncherAttempt {
                    outcome: Err(format!(
                        "disc {} does not verify against the catalogue",
                        canonical.display()
                    )),
                    known_files,
                };
            };
            game_index
        };
        disc_game_indexes.push(disc_game_index);
    }

    if disc_game_indexes.len() < 2 {
        // A single-entry M3U is not a multidisc release this module needs
        // to combine specially - left for the ordinary single-file/CUE/GDI
        // passes to handle on their own. Its one disc is not excluded:
        // nothing about this playlist was rejected.
        return LauncherAttempt {
            outcome: Ok(None),
            known_files,
        };
    }

    let mut base_titles = BTreeSet::new();
    for &index in &disc_game_indexes {
        let name = &dat.games[index].name;
        match multidisc_group_key(name) {
            Some(token) => {
                base_titles.insert(token.base_title);
            }
            None => {
                return LauncherAttempt {
                    outcome: Err(format!(
                        "\"{name}\" has no recognised multidisc \"(Disc N of M)\" token"
                    )),
                    known_files,
                };
            }
        }
    }
    if base_titles.len() != 1 {
        return LauncherAttempt {
            outcome: Err(
                "referenced discs belong to different multidisc releases, not one".to_string(),
            ),
            known_files,
        };
    }

    LauncherAttempt {
        outcome: Ok(Some(DatArchiveMatch {
            archive_path: launcher.to_path_buf(),
            dat_entry_index: disc_game_indexes[0],
            companion_paths: known_files.clone(),
        })),
        known_files,
    }
}

#[cfg(test)]
mod tests;
