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

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::dat::index::DatIndex;
use crate::dat::model::ParsedDat;
use crate::identity_source::hashing::hash_file;
use crate::safe_read::TrustedRoots;

use super::DatArchiveMatch;

/// Hashes every candidate and keeps only the ones whose SHA-1 resolves to
/// exactly one game in `dat`. Unreadable files, hash refusals, and
/// hash-collisions across more than one distinct game are silently dropped -
/// never guessed.
pub fn match_loose_files_against_dat(
    dat: &ParsedDat,
    candidates: &[PathBuf],
    trusted: &TrustedRoots,
    cancel: &AtomicBool,
) -> Vec<DatArchiveMatch> {
    let index = DatIndex::build(dat);
    let mut matches = Vec::new();
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
            let game_index = *distinct_games.iter().next().expect("len() == 1");
            matches.push(DatArchiveMatch {
                archive_path: candidate.clone(),
                dat_entry_index: game_index,
            });
        }
    }
    matches
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat::model::{DatEcosystem, DatFormat, DatGameEntry, DatRomEntry, DatSource};

    fn dat_with_one_rom(sha1: &str) -> ParsedDat {
        ParsedDat {
            source: DatSource {
                format: DatFormat::Logiqx,
                ecosystem: DatEcosystem::NoIntro,
                file_path: "synthetic.dat".to_string(),
                name: Some("Synthetic".to_string()),
                description: None,
                version: None,
                author: None,
                homepage: None,
                clrmamepro_header: None,
                entry_count: 1,
                rom_count: 1,
                parse_warnings: Vec::new(),
            },
            games: vec![DatGameEntry {
                name: "Game (Europe)".to_string(),
                roms: vec![DatRomEntry {
                    name: "game.bin".to_string(),
                    size_bytes: Some(4),
                    sha1: Some(sha1.to_string()),
                    ..DatRomEntry::default()
                }],
                ..DatGameEntry::default()
            }],
        }
    }

    #[test]
    fn a_hash_verified_file_is_matched_to_its_game_index() {
        let temp = tempfile::tempdir().expect("temp dir");
        let file = temp.path().join("loose.bin");
        std::fs::write(&file, b"test").unwrap();
        // SHA-1 of b"test".
        let dat = dat_with_one_rom("a94a8fe5ccb19ba61c4c0873d391e987982fbbd3");

        let matches = match_loose_files_against_dat(
            &dat,
            &[file.clone()],
            &TrustedRoots::from_paths([temp.path()]),
            &AtomicBool::new(false),
        );

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].archive_path, file);
        assert_eq!(matches[0].dat_entry_index, 0);
    }

    #[test]
    fn an_unmatched_file_is_silently_dropped() {
        let temp = tempfile::tempdir().expect("temp dir");
        let file = temp.path().join("unknown.bin");
        std::fs::write(&file, b"not in the catalogue").unwrap();
        let dat = dat_with_one_rom("a94a8fe5ccb19ba61c4c0873d391e987982fbbd3");

        let matches = match_loose_files_against_dat(
            &dat,
            &[file],
            &TrustedRoots::from_paths([temp.path()]),
            &AtomicBool::new(false),
        );

        assert!(matches.is_empty());
    }
}
