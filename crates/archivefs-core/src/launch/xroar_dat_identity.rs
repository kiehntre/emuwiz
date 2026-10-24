//! Verified Dragon/CoCo cassette identity from exact TOSEC catalogue evidence.
//!
//! `.cas` is a container several systems share, and a structurally valid tape
//! proves nothing about which machine family it belongs to. The only thing
//! accepted here is an exact whole-file match (size, SHA-1, MD5 and CRC32 all
//! agree with the catalogue entry) in an imported TOSEC catalogue whose own
//! header names a reviewed Dragon or Tandy Color Computer cassette system.
//!
//! What this does and does not establish:
//! - platform family: "Dragon / Tandy CoCo" - from the matching catalogue.
//! - dump identity: the exact hashes. The title is display-only metadata.
//! - NOT machine choice (Dragon 32/64 vs CoCo 2/3 stays an explicit setting),
//!   NOT media completeness, NOT that the game boots or loads.
//!
//! Fail-closed: a hash that also appears in a catalogue for any other or
//! unreviewed system, an entry whose size/checksums disagree, a non-cassette
//! catalogue, or an unclassifiable catalogue name all refuse. Nothing is
//! inferred from the file name or from tape structure.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use md5::Md5;
use sha1::Sha1;
use sha1::digest::Digest;

use crate::dat::model::ChecksumAlgorithm;
use crate::dat::tosec_release_pack::{TosecMediaType, classify_tosec_catalogue_name};
use crate::identity_source::hashing::Crc32;
use crate::identity_source::tosec::{
    ImportedTosecSource, lookup_tosec, parse_tosec_filename_metadata,
};
use crate::launch::planning::{
    CanonicalIdentityStatus, LaunchContainerKind, LaunchContentKind, LaunchContentRef,
    ResolvedIdentity,
};
use crate::launch::process_spawn::{CapturedFileIdentity, capture_file_identity};
use crate::launch::xroar_command::xroar_media_format;

pub const DRAGON_COCO_PLATFORM_ID: &str = "Dragon / Tandy CoCo";
const MAX_CASSETTE_BYTES: u64 = 8 * 1024 * 1024;

/// The reviewed TOSEC system names (exact, trimmed) for the XRoar family.
const REVIEWED_SYSTEMS: &[(&str, DragonCocoCatalogue)] = &[
    ("Dragon Data Dragon", DragonCocoCatalogue::Dragon),
    (
        "Tandy Radio Shack TRS-80 Color Computer",
        DragonCocoCatalogue::TandyColorComputer,
    ),
];

/// Which catalogue family matched. Informational only: it never selects the
/// XRoar machine, and the two families share some identical dumps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DragonCocoCatalogue {
    Dragon,
    TandyColorComputer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DragonCocoCatalogueMatch {
    pub catalogue: DragonCocoCatalogue,
    pub catalogue_name: String,
    pub catalogue_version: Option<String>,
    pub catalogue_artifact_sha256: String,
    pub entry_name: String,
    /// Display-only; never game identity.
    pub title: String,
    pub dump_flags: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DragonCocoCassetteEvidence {
    pub path: PathBuf,
    /// The file as read; launch preflight compares against it again.
    pub file_identity: CapturedFileIdentity,
    pub size: u64,
    pub sha1: String,
    pub md5: String,
    pub crc32: String,
    pub matches: Vec<DragonCocoCatalogueMatch>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragonCocoRefusalKind {
    NotCasFile,
    Unreadable,
    NotRegularFile,
    TooLarge,
    ChangedWhileReading,
    NoTosecCatalogue,
    /// An enabled catalogue could not be consulted, so a cross-system match
    /// cannot be ruled out.
    CatalogueIncomplete,
    NoExactMatch,
    EntryDisagrees,
    ForeignCatalogueMatch,
    NotCassetteCatalogue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DragonCocoRefusal {
    pub kind: DragonCocoRefusalKind,
    pub detail: String,
}

fn refuse(kind: DragonCocoRefusalKind, detail: impl Into<String>) -> DragonCocoRefusal {
    DragonCocoRefusal {
        kind,
        detail: detail.into(),
    }
}

struct Hashes {
    size: u64,
    sha1: String,
    md5: String,
    crc32: String,
    identity: CapturedFileIdentity,
}

fn hash_cassette(path: &Path) -> Result<Hashes, DragonCocoRefusal> {
    let unreadable = |_| {
        refuse(
            DragonCocoRefusalKind::Unreadable,
            "the cassette file could not be read",
        )
    };
    let (before_meta, before) = capture_file_identity(path).map_err(unreadable)?;
    if before_meta.file_type().is_symlink() || !before_meta.is_file() {
        return Err(refuse(
            DragonCocoRefusalKind::NotRegularFile,
            "the cassette is not a regular file (links and folders are refused)",
        ));
    }
    if before.size > MAX_CASSETTE_BYTES {
        return Err(refuse(
            DragonCocoRefusalKind::TooLarge,
            "the cassette exceeds the 8 MiB verification limit",
        ));
    }
    let mut bytes = Vec::new();
    // Never follow a link or block on a FIFO swapped in after the check above.
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(unreadable)?
    };
    if !file.metadata().map_err(unreadable)?.is_file() {
        return Err(refuse(
            DragonCocoRefusalKind::NotRegularFile,
            "the cassette is not a regular file (links and folders are refused)",
        ));
    }
    file.take(MAX_CASSETTE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    let (_, after) = capture_file_identity(path).map_err(unreadable)?;
    if after != before || bytes.len() as u64 != before.size {
        return Err(refuse(
            DragonCocoRefusalKind::ChangedWhileReading,
            "the cassette changed while it was being read",
        ));
    }
    let mut crc = Crc32::new();
    crc.update(&bytes);
    Ok(Hashes {
        size: before.size,
        sha1: hex(&Sha1::digest(&bytes)),
        md5: hex(&Md5::digest(&bytes)),
        crc32: crc.finish_hex(),
        identity: before,
    })
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// Verifies `path` against the supplied imported TOSEC catalogues.
///
/// `sources` should be every TOSEC catalogue the user has registered, not only
/// the Dragon/CoCo ones: a match in any other system's catalogue is what makes
/// a shared-container file ambiguous, and that must refuse.
pub fn verify_dragon_coco_cassette(
    path: &Path,
    sources: &[ImportedTosecSource],
) -> Result<DragonCocoCassetteEvidence, DragonCocoRefusal> {
    if xroar_media_format(path).is_none() {
        return Err(refuse(
            DragonCocoRefusalKind::NotCasFile,
            "XRoar launch supports only direct .cas tape images",
        ));
    }
    if sources.is_empty() {
        return Err(refuse(
            DragonCocoRefusalKind::NoTosecCatalogue,
            "no TOSEC catalogue is registered, so this file's system cannot be verified",
        ));
    }
    let hashes = hash_cassette(path)?;
    let mut matches = Vec::new();
    let mut media_refusal = None;
    for source in sources {
        let entries = lookup_tosec(&source.index, ChecksumAlgorithm::Sha1, &hashes.sha1);
        if entries.is_empty() {
            continue;
        }
        let classification = classify_tosec_catalogue_name(&source.system_name);
        let reviewed = REVIEWED_SYSTEMS
            .iter()
            .find(|(system, _)| classification.confident && *system == classification.system);
        let Some((_, catalogue)) = reviewed else {
            return Err(refuse(
                DragonCocoRefusalKind::ForeignCatalogueMatch,
                format!(
                    "this file's hash is also catalogued under '{}', which is not a Dragon/CoCo \
                     system, so its system is ambiguous",
                    source.system_name
                ),
            ));
        };
        for rom in entries {
            let declared = |algorithm: ChecksumAlgorithm| {
                rom.checksums
                    .iter()
                    .filter(|checksum| checksum.algorithm == algorithm)
                    .map(|checksum| checksum.value.as_str())
                    .collect::<Vec<_>>()
            };
            let agrees = |algorithm: ChecksumAlgorithm, value: &str| {
                let values = declared(algorithm);
                !values.is_empty() && values.iter().all(|declared| *declared == value)
            };
            if rom.size_bytes != Some(hashes.size)
                || !agrees(ChecksumAlgorithm::Md5, &hashes.md5)
                || !agrees(ChecksumAlgorithm::Crc32, &hashes.crc32)
            {
                return Err(refuse(
                    DragonCocoRefusalKind::EntryDisagrees,
                    format!(
                        "'{}' matches on SHA-1 but its size or other checksums disagree",
                        source.system_name
                    ),
                ));
            }
            if classification.media != TosecMediaType::Tape {
                media_refusal = Some(source.system_name.clone());
                continue;
            }
            let metadata = parse_tosec_filename_metadata(&rom.game_name);
            matches.push(DragonCocoCatalogueMatch {
                catalogue: *catalogue,
                catalogue_name: source.system_name.clone(),
                catalogue_version: source.upstream_version.clone(),
                catalogue_artifact_sha256: source.artifact_sha256.clone(),
                entry_name: rom.game_name.clone(),
                title: if metadata.title.is_empty() {
                    rom.game_name.clone()
                } else {
                    metadata.title
                },
                dump_flags: metadata.flags.labels(),
            });
        }
    }
    if matches.is_empty() {
        return Err(match media_refusal {
            Some(name) => refuse(
                DragonCocoRefusalKind::NotCassetteCatalogue,
                format!("the matching catalogue '{name}' does not catalogue cassette images"),
            ),
            None => refuse(
                DragonCocoRefusalKind::NoExactMatch,
                "no registered TOSEC Dragon/CoCo catalogue contains this exact file",
            ),
        });
    }
    Ok(DragonCocoCassetteEvidence {
        path: path.to_path_buf(),
        file_identity: hashes.identity,
        size: hashes.size,
        sha1: hashes.sha1,
        md5: hashes.md5,
        crc32: hashes.crc32,
        matches,
    })
}

impl DragonCocoCassetteEvidence {
    /// Family-level identity keyed by the verified dump hash.
    pub fn identity_status(&self) -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: DRAGON_COCO_PLATFORM_ID.into(),
            game_key: self.sha1.clone(),
        })
    }

    pub fn content_ref(&self) -> LaunchContentRef {
        LaunchContentRef {
            kind: Some(LaunchContentKind::Executable),
            container: Some(LaunchContainerKind::PlainFile),
            resolved_path: Some(self.path.clone()),
            requires_mount: false,
            provenance: "exact whole-file match in a TOSEC Dragon/CoCo cassette catalogue".into(),
        }
    }

    /// The catalogues that matched, for display ("Dragon", "Tandy Color Computer").
    pub fn catalogue_summary(&self) -> String {
        let mut names: Vec<&str> = self
            .matches
            .iter()
            .map(|entry| entry.catalogue_name.as_str())
            .collect();
        names.sort_unstable();
        names.dedup();
        names.join("; ")
    }

    /// Manual steps: XRoar is only given the tape, never told to run it.
    pub fn loading_steps(&self) -> Vec<String> {
        vec![
            "XRoar opens with this cassette inserted; it does not start the tape by itself.".into(),
            "Type CLOAD then RUN for a BASIC program, or CLOADM then EXEC for machine code.".into(),
            "The machine is the one selected in Emulator Setup. The catalogue does not choose \
             it, and a tape for the other machine may not run."
                .into(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity_source::tosec::import_tosec_dat;

    const GAME: &[u8] = b"synthetic cassette payload 0123456789";

    fn digests(bytes: &[u8]) -> (String, String, String) {
        (
            hex(&Sha1::digest(bytes)),
            hex(&Md5::digest(bytes)),
            Crc32::of(bytes),
        )
    }

    fn dat(name: &str, entry: &str, bytes: &[u8], size: u64) -> String {
        let (sha1, md5, crc) = digests(bytes);
        format!(
            r#"<?xml version="1.0"?>
<datafile><header><name>{name}</name><description>{name} (TOSEC-v2025-01-15)</description><category>TOSEC</category><version>2025-01-15</version><author>Tester</author><homepage>TOSEC</homepage></header>
<game name="{entry}"><description>{entry}</description><rom name="{entry}.cas" size="{size}" crc="{crc}" md5="{md5}" sha1="{sha1}"/></game></datafile>"#,
            size = size
        )
    }

    fn source(dir: &Path, file: &str, name: &str, bytes: &[u8], size: u64) -> ImportedTosecSource {
        let path = dir.join(file);
        fs::write(&path, dat(name, "Example Game (1984)(Acme)", bytes, size)).unwrap();
        import_tosec_dat(&path).unwrap()
    }

    fn dragon(dir: &Path) -> ImportedTosecSource {
        source(
            dir,
            "dragon.dat",
            "Dragon Data Dragon - Games - [CAS]",
            GAME,
            GAME.len() as u64,
        )
    }

    fn cassette(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn kind(
        result: Result<DragonCocoCassetteEvidence, DragonCocoRefusal>,
    ) -> DragonCocoRefusalKind {
        result.unwrap_err().kind
    }

    #[test]
    fn exact_dragon_catalogue_match_resolves_family_identity_only() {
        let dir = tempfile::tempdir().unwrap();
        let tape = cassette(dir.path(), "whatever name.cas", GAME);
        let evidence = verify_dragon_coco_cassette(&tape, &[dragon(dir.path())]).unwrap();
        assert_eq!(evidence.matches[0].catalogue, DragonCocoCatalogue::Dragon);
        assert_eq!(evidence.matches[0].title, "Example Game");
        match evidence.identity_status() {
            CanonicalIdentityStatus::Resolved(id) => {
                assert_eq!(id.platform_id, DRAGON_COCO_PLATFORM_ID);
                assert_eq!(id.game_key, evidence.sha1);
            }
            other => panic!("{other:?}"),
        }
        assert!(evidence.loading_steps().len() >= 3);
    }

    #[test]
    fn matching_name_without_matching_bytes_is_not_identity() {
        let dir = tempfile::tempdir().unwrap();
        let tape = cassette(
            dir.path(),
            "Example Game (1984)(Acme).cas",
            b"different bytes",
        );
        assert_eq!(
            kind(verify_dragon_coco_cassette(&tape, &[dragon(dir.path())])),
            DragonCocoRefusalKind::NoExactMatch
        );
    }

    #[test]
    fn dragon_and_coco_catalogues_both_match_without_choosing_a_machine() {
        let dir = tempfile::tempdir().unwrap();
        let coco = source(
            dir.path(),
            "coco.dat",
            "Tandy Radio Shack TRS-80 Color Computer - Games - [CAS]",
            GAME,
            GAME.len() as u64,
        );
        let tape = cassette(dir.path(), "t.cas", GAME);
        let evidence = verify_dragon_coco_cassette(&tape, &[dragon(dir.path()), coco]).unwrap();
        assert_eq!(evidence.matches.len(), 2);
        assert!(evidence.catalogue_summary().contains("Color Computer"));
    }

    #[test]
    fn a_match_in_another_systems_catalogue_makes_the_file_ambiguous() {
        let dir = tempfile::tempdir().unwrap();
        let msx = source(
            dir.path(),
            "msx.dat",
            "MSX MSX - Games - [CAS]",
            GAME,
            GAME.len() as u64,
        );
        let tape = cassette(dir.path(), "t.cas", GAME);
        // Order must not matter: the foreign match refuses either way.
        for sources in [
            vec![dragon(dir.path()), msx.clone()],
            vec![msx.clone(), dragon(dir.path())],
        ] {
            assert_eq!(
                kind(verify_dragon_coco_cassette(&tape, &sources)),
                DragonCocoRefusalKind::ForeignCatalogueMatch
            );
        }
        assert_eq!(
            kind(verify_dragon_coco_cassette(&tape, &[msx])),
            DragonCocoRefusalKind::ForeignCatalogueMatch
        );
    }

    #[test]
    fn an_unreviewed_neighbouring_tandy_system_is_not_dragon_coco() {
        let dir = tempfile::tempdir().unwrap();
        let model3 = source(
            dir.path(),
            "m3.dat",
            "Tandy Radio Shack TRS-80 Model III - Games - [CAS]",
            GAME,
            GAME.len() as u64,
        );
        let tape = cassette(dir.path(), "t.cas", GAME);
        assert_eq!(
            kind(verify_dragon_coco_cassette(&tape, &[model3])),
            DragonCocoRefusalKind::ForeignCatalogueMatch
        );
    }

    #[test]
    fn size_or_checksum_disagreement_in_the_entry_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let wrong_size = source(
            dir.path(),
            "d.dat",
            "Dragon Data Dragon - Games - [CAS]",
            GAME,
            GAME.len() as u64 + 1,
        );
        let tape = cassette(dir.path(), "t.cas", GAME);
        assert_eq!(
            kind(verify_dragon_coco_cassette(&tape, &[wrong_size])),
            DragonCocoRefusalKind::EntryDisagrees
        );
    }

    #[test]
    fn a_non_cassette_catalogue_is_not_cassette_identity() {
        let dir = tempfile::tempdir().unwrap();
        let disk = source(
            dir.path(),
            "disk.dat",
            "Dragon Data Dragon - Games - [DSK]",
            GAME,
            GAME.len() as u64,
        );
        let tape = cassette(dir.path(), "t.cas", GAME);
        assert_eq!(
            kind(verify_dragon_coco_cassette(&tape, &[disk])),
            DragonCocoRefusalKind::NotCassetteCatalogue
        );
    }

    #[test]
    fn other_media_no_catalogue_links_and_missing_files_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let sources = [dragon(dir.path())];
        let dsk = cassette(dir.path(), "game.dsk", GAME);
        assert_eq!(
            kind(verify_dragon_coco_cassette(&dsk, &sources)),
            DragonCocoRefusalKind::NotCasFile
        );
        let tape = cassette(dir.path(), "t.cas", GAME);
        assert_eq!(
            kind(verify_dragon_coco_cassette(&tape, &[])),
            DragonCocoRefusalKind::NoTosecCatalogue
        );
        let link = dir.path().join("link.cas");
        std::os::unix::fs::symlink(&tape, &link).unwrap();
        assert_eq!(
            kind(verify_dragon_coco_cassette(&link, &sources)),
            DragonCocoRefusalKind::NotRegularFile
        );
        assert_eq!(
            kind(verify_dragon_coco_cassette(
                &dir.path().join("missing.cas"),
                &sources
            )),
            DragonCocoRefusalKind::Unreadable
        );
    }

    #[test]
    fn a_changed_file_no_longer_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let sources = [dragon(dir.path())];
        let tape = cassette(dir.path(), "t.cas", GAME);
        let before = verify_dragon_coco_cassette(&tape, &sources).unwrap();
        fs::write(&tape, b"edited after verification").unwrap();
        assert_eq!(
            kind(verify_dragon_coco_cassette(&tape, &sources)),
            DragonCocoRefusalKind::NoExactMatch
        );
        assert_ne!(
            capture_file_identity(&tape).unwrap().1,
            before.file_identity
        );
    }

    /// Run with `EMUWIZ_TOSEC_SAMPLE_DIR` pointing at the extracted official
    /// `[CAS]` DATs: checks the reviewed names classify exactly as assumed.
    #[test]
    #[ignore = "needs the official TOSEC [CAS] DATs in EMUWIZ_TOSEC_SAMPLE_DIR"]
    fn official_catalogue_names_classify_as_reviewed() {
        let dir = std::env::var("EMUWIZ_TOSEC_SAMPLE_DIR").unwrap();
        let mut reviewed = 0;
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let Ok(source) = import_tosec_dat(&path) else {
                continue;
            };
            let class = classify_tosec_catalogue_name(&source.system_name);
            let is_reviewed = REVIEWED_SYSTEMS.iter().any(|(s, _)| *s == class.system);
            if source.system_name.contains("Dragon Data Dragon - Games")
                || source.system_name.contains("Color Computer - Games")
            {
                assert!(is_reviewed && class.confident, "{}", source.system_name);
                assert_eq!(class.media, TosecMediaType::Tape, "{}", source.system_name);
                reviewed += 1;
            } else if !source
                .system_name
                .starts_with("Dragon Data Dragon - Applications")
            {
                assert!(!is_reviewed, "{}", source.system_name);
            }
        }
        assert_eq!(reviewed, 2);
    }
}
