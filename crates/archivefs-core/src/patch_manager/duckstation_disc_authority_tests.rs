//! Single-disc proof: the four acceptance cases (DAT-backed, non-DAT verified,
//! known multi-disc, uncertain) plus the hierarchy and parser safety rules.

use std::path::{Path, PathBuf};

use super::*;
use crate::media_set::*;

// ---------------------------------------------------------------------------
// A synthetic DuckStation cache in the verified v33 layout
// ---------------------------------------------------------------------------

fn put_string(out: &mut Vec<u8>, text: &str) {
    out.extend((text.len() as u32).to_le_bytes());
    out.extend(text.as_bytes());
}

fn cache_bytes(version: u32, sets: &[&[&str]], games: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(b"HLCE");
    out.extend(version.to_le_bytes());
    out.extend(0x1122_3344_u64.to_le_bytes());
    out.extend(0x1122_3344_u64.to_le_bytes());
    out.extend((sets.len() as u32).to_le_bytes());
    out.extend((games.len() as u32).to_le_bytes());
    out.extend(0_u32.to_le_bytes());
    for (index, serials) in sets.iter().enumerate() {
        put_string(&mut out, &format!("Set {index}"));
        put_string(&mut out, "");
        put_string(&mut out, "");
        put_string(&mut out, &format!("Set {index} (Europe)"));
        out.extend((serials.len() as u32).to_le_bytes());
        for serial in *serials {
            put_string(&mut out, serial);
        }
    }
    for serial in games {
        put_string(&mut out, serial);
        put_string(&mut out, "A Title");
        out.extend([0_u8; 24]);
    }
    out
}

fn database(sets: &[&[&str]], games: &[&str]) -> DuckStationDatabaseAuthority {
    let path = PathBuf::from("/profile/cache/gamedb.cache");
    DuckStationDatabaseAuthority {
        path: path.clone(),
        database: DuckStationGameDatabase::parse_with_floor(
            &path,
            &cache_bytes(33, sets, games),
            1,
            1,
        ),
    }
}

fn lenient(path: &Path, data: &[u8]) -> Result<DuckStationGameDatabase, String> {
    DuckStationGameDatabase::parse_with_floor(path, data, 1, 1)
}

fn query<'a>(serial: Option<&'a str>, archive: &'a Path) -> DiscQuery<'a> {
    DiscQuery { serial, archive }
}

// ---------------------------------------------------------------------------
// Trusted DAT media sets (as the library persists them)
// ---------------------------------------------------------------------------

fn dat_record(path: &Path, ordinal: u16, total: u16) -> MediaRecord {
    let mut record = media_record(path, Some("PlayStation"));
    record.availability = MediaAvailability::Observed;
    let mut evidence = MediaEvidence::new(EvidenceKind::TrustedDat, "test-topology");
    evidence.provenance.version = Some("1".into());
    evidence.release = Some(IdentityKey::new("test-release", "orbit"));
    evidence.medium = Some(IdentityKey::new("test-medium", format!("disc-{ordinal}")));
    evidence.equivalence = Equivalence::AuthorityMapping;
    evidence.ordinal = Some(MediaOrdinal {
        number: ordinal,
        unit: OrdinalUnit::Disc,
    });
    evidence.expected_count = Some(ExpectedCount {
        count: total,
        unit: OrdinalUnit::Disc,
    });
    record.evidence.push(evidence);
    record
}

fn dat_sets(paths: &[&Path], total: u16) -> Vec<MediaSet> {
    let records = paths
        .iter()
        .enumerate()
        .map(|(index, path)| dat_record(path, index as u16 + 1, total))
        .collect();
    resolve_index(index_media(records)).sets
}

// ---------------------------------------------------------------------------
// A. DAT-backed verified single-disc game -> allowed
// ---------------------------------------------------------------------------

#[test]
fn a_dat_backed_single_disc_game_is_proven() {
    let archive = Path::new("/roms/ps/orbit.chd");
    let sets = dat_sets(&[archive], 1);
    let dat = TrustedDatAuthority { sets: &sets };
    let proof = prove_disc_topology(&query(Some("SCUS-94601"), archive), &[&dat]);
    assert!(
        matches!(
            &proof,
            DiscTopologyProof::SingleDisc {
                authority: DiscAuthority::TrustedDatSet { .. }
            }
        ),
        "{proof:?}"
    );
    assert!(proof.refusal().is_none());
}

// ---------------------------------------------------------------------------
// B. Non-DAT, independently verified single-disc game -> allowed
// ---------------------------------------------------------------------------

#[test]
fn a_non_dat_game_is_proven_by_the_duckstation_game_database() {
    // No DAT record at all for this image (a CHD that never matched a hash).
    let archive = Path::new("/roms/ps/orbit.chd");
    let no_sets: Vec<MediaSet> = Vec::new();
    let dat = TrustedDatAuthority { sets: &no_sets };
    let db = database(
        &[&["SLPS-00071", "SLPS-00072"]],
        &["SCUS-94601", "SLPS-00071", "SLPS-00072"],
    );
    let proof = prove_disc_topology(&query(Some("SCUS-94601"), archive), &[&dat, &db]);
    match &proof {
        DiscTopologyProof::SingleDisc {
            authority: DiscAuthority::DuckStationGameDatabase { version, .. },
        } => assert_eq!(*version, 33),
        other => panic!("expected the game database to prove it, got {other:?}"),
    }
    assert_eq!(proof.topology(), DuckStationDiscTopology::SingleDisc);
    assert!(proof.describe().contains("DuckStation's game database"));
}

#[test]
fn the_dat_route_is_not_required_and_not_preferred_over_a_conflict() {
    let archive = Path::new("/roms/ps/orbit.chd");
    let sets = dat_sets(&[archive], 1);
    let dat = TrustedDatAuthority { sets: &sets };
    // DAT says one disc, the emulator's database says it belongs to a set.
    let db = database(
        &[&["SCUS-94601", "SCUS-94602"]],
        &["SCUS-94601", "SCUS-94602"],
    );
    let proof = prove_disc_topology(&query(Some("SCUS-94601"), archive), &[&dat, &db]);
    assert!(
        matches!(proof, DiscTopologyProof::Conflict { .. }),
        "{proof:?}"
    );
    assert!(matches!(
        proof.refusal(),
        Some(DuckStationNativeRefusal::DiscAuthoritiesConflict { .. })
    ));
}

// ---------------------------------------------------------------------------
// C. Known multi-disc game -> refused
// ---------------------------------------------------------------------------

#[test]
fn a_known_multi_disc_game_is_refused_by_either_authority() {
    let archive = Path::new("/roms/ps/ff7-1.chd");
    // The database knows the set.
    let db = database(
        &[&["SCUS-94163", "SCUS-94164", "SCUS-94165"]],
        &["SCUS-94163", "SCUS-94164", "SCUS-94165"],
    );
    let proof = prove_disc_topology(&query(Some("SCUS-94164"), archive), &[&db]);
    match &proof {
        DiscTopologyProof::MultiDisc { serials, .. } => assert_eq!(serials.len(), 3),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        proof.refusal(),
        Some(DuckStationNativeRefusal::UnsupportedMultiDisc)
    );
    // A trusted DAT set with two members says the same.
    let other = Path::new("/roms/ps/ff7-2.chd");
    let sets = dat_sets(&[archive, other], 2);
    let dat = TrustedDatAuthority { sets: &sets };
    let proof = prove_disc_topology(&query(Some("SCUS-94163"), archive), &[&dat]);
    assert!(
        matches!(proof, DiscTopologyProof::MultiDisc { .. }),
        "{proof:?}"
    );
}

// ---------------------------------------------------------------------------
// D. Uncertain disc count -> refused, naming WHICH proof is missing
// ---------------------------------------------------------------------------

#[test]
fn an_uncertain_disc_count_is_refused_and_names_the_missing_proof() {
    let archive = Path::new("/roms/ps/mystery.chd");
    let no_sets: Vec<MediaSet> = Vec::new();
    let dat = TrustedDatAuthority { sets: &no_sets };
    // The database is healthy but has never heard of this serial.
    let db = database(&[&["SLPS-00071", "SLPS-00072"]], &["SCUS-94601"]);
    let proof = prove_disc_topology(&query(Some("SLUS-99999"), archive), &[&dat, &db]);
    let DiscTopologyProof::Unproven { missing } = &proof else {
        panic!("{proof:?}");
    };
    assert!(missing.contains(&MissingDiscProof::NoTrustedDatSet));
    assert!(
        missing
            .iter()
            .any(|item| matches!(item, MissingDiscProof::SerialNotInGameDatabase { .. }))
    );
    let text = proof.refusal().unwrap().explain();
    assert!(text.contains("no trusted DAT set"), "{text}");
    assert!(text.contains("does not list this serial"), "{text}");
    // Not "a DAT match is mandatory": the message lists every route tried.
    assert!(!text.contains("must be matched"));
}

#[test]
fn no_verified_serial_means_nothing_can_be_proven_even_if_an_authority_would_say_single() {
    let archive = Path::new("/roms/ps/orbit.chd");
    let sets = dat_sets(&[archive], 1);
    let dat = TrustedDatAuthority { sets: &sets };
    let proof = prove_disc_topology(&query(None, archive), &[&dat]);
    let DiscTopologyProof::Unproven { missing } = proof else {
        panic!("a single-disc claim needs the disc's verified serial");
    };
    assert!(missing.contains(&MissingDiscProof::NoVerifiedSerial));
}

#[test]
fn an_unavailable_database_is_a_stated_reason_not_a_silent_skip() {
    let archive = Path::new("/roms/ps/orbit.chd");
    let db = DuckStationDatabaseAuthority {
        path: PathBuf::from("/profile/cache/gamedb.cache"),
        database: Err("No such file".into()),
    };
    let proof = prove_disc_topology(&query(Some("SCUS-94601"), archive), &[&db]);
    let DiscTopologyProof::Unproven { missing } = proof else {
        panic!();
    };
    assert!(
        matches!(&missing[0], MissingDiscProof::GameDatabaseUnavailable { reason } if reason.contains("No such file"))
    );
}

#[test]
fn filenames_and_disc_text_are_never_evidence() {
    // "(Disc 1)" in the name and a lone file prove nothing without an authority.
    let archive = Path::new("/roms/ps/Some Game (USA) (Disc 1).chd");
    let proof = prove_disc_topology(&query(Some("SCUS-94601"), archive), &[]);
    assert!(matches!(proof, DiscTopologyProof::Unproven { .. }));
}

// ---------------------------------------------------------------------------
// The cache reader fails closed
// ---------------------------------------------------------------------------

#[test]
fn the_cache_reader_rejects_what_it_was_not_verified_against() {
    let path = Path::new("/x");
    assert!(lenient(path, b"not a cache").is_err());
    let wrong_version = cache_bytes(34, &[&["SLPS-00071", "SLPS-00072"]], &["SLPS-00071"]);
    assert!(
        lenient(path, &wrong_version)
            .unwrap_err()
            .contains("version 34")
    );
    let mut truncated = cache_bytes(33, &[&["SLPS-00071", "SLPS-00072"]], &["SLPS-00071"]);
    truncated.truncate(60);
    assert!(lenient(path, &truncated).is_err());
    // A "serial" that is not a PS1 serial inside a set poisons the whole file.
    let bad = cache_bytes(33, &[&["SLPS-00071", "not-a-serial"]], &["SLPS-00071"]);
    assert!(lenient(path, &bad).is_err());
    // A database that lists no games proves nothing.
    let empty = cache_bytes(33, &[&["SLPS-00071", "SLPS-00072"]], &[]);
    assert!(lenient(path, &empty).is_err());
}

#[test]
fn an_implausibly_small_database_is_refused_so_its_silence_proves_nothing() {
    let path = Path::new("/x");
    let small = cache_bytes(33, &[&["SLPS-00071", "SLPS-00072"]], &["SCUS-94601"]);
    let error = DuckStationGameDatabase::parse(path, &small).unwrap_err();
    assert!(error.contains("looks incomplete"), "{error}");
    // A database of plausible size is accepted by the production floors.
    let sets: Vec<Vec<String>> = (0..MIN_DISC_SETS)
        .map(|i| {
            vec![
                format!("SLPS-{:05}", i * 2),
                format!("SLPS-{:05}", i * 2 + 1),
            ]
        })
        .collect();
    let set_refs: Vec<Vec<&str>> = sets
        .iter()
        .map(|set| set.iter().map(String::as_str).collect())
        .collect();
    let set_slices: Vec<&[&str]> = set_refs.iter().map(Vec::as_slice).collect();
    let games: Vec<String> = (0..MIN_LISTED_GAMES)
        .map(|i| format!("SCUS-{i:05}"))
        .collect();
    let game_refs: Vec<&str> = games.iter().map(String::as_str).collect();
    let big = cache_bytes(33, &set_slices, &game_refs);
    let database = DuckStationGameDatabase::parse(path, &big).unwrap();
    assert_eq!(database.disc_sets.len(), MIN_DISC_SETS);
}

#[test]
fn authorities_that_disagree_or_cannot_speak_never_produce_a_single_disc_claim() {
    let archive = Path::new("/roms/ps/x.chd");
    // Unknown game + unsupported-version database + no DAT: unproven, all reasons listed.
    let unsupported = DuckStationDatabaseAuthority {
        path: PathBuf::from("/profile/cache/gamedb.cache"),
        database: lenient(
            Path::new("/x"),
            &cache_bytes(34, &[&["SLPS-00071", "SLPS-00072"]], &["SCUS-94601"]),
        ),
    };
    let no_sets: Vec<MediaSet> = Vec::new();
    let dat = TrustedDatAuthority { sets: &no_sets };
    let proof = prove_disc_topology(&query(Some("SCUS-94601"), archive), &[&dat, &unsupported]);
    let DiscTopologyProof::Unproven { missing } = proof else {
        panic!();
    };
    assert!(missing
        .iter()
        .any(|m| matches!(m, MissingDiscProof::GameDatabaseUnavailable { reason } if reason.contains("version 34"))));
    // A DAT set that is not a proven one-disc set stays silent.
    let mut unproven = dat_sets(&[archive], 1);
    unproven[0].confidence = MediaSetConfidence::Likely;
    let weak = TrustedDatAuthority { sets: &unproven };
    let proof = prove_disc_topology(&query(Some("SCUS-94601"), archive), &[&weak]);
    assert!(
        matches!(proof, DiscTopologyProof::Unproven { .. }),
        "{proof:?}"
    );
}

#[test]
fn the_cache_path_honours_a_cache_folder_setting() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        DuckStationGameDatabase::path_in_profile(dir.path()),
        dir.path().join("cache/gamedb.cache")
    );
    std::fs::write(
        dir.path().join("settings.ini"),
        "[Folders]\nCache = my-cache\n",
    )
    .unwrap();
    assert_eq!(
        DuckStationGameDatabase::path_in_profile(dir.path()),
        dir.path().join("my-cache/gamedb.cache")
    );
}

// ---------------------------------------------------------------------------
// The real machine, read-only (ignored by default)
// ---------------------------------------------------------------------------

/// Reads the real portable profile's `cache/gamedb.cache` and a sample of the
/// real PS1 library's discs (SYSTEM.CNF only), and reports how many would be
/// allowed. Writes nothing.
#[test]
#[ignore = "inspects the real machine; read-only"]
fn real_library_disc_proof_report() {
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    let profile = home.join("Applications/DuckStation");
    let authority = DuckStationDatabaseAuthority::for_profile(&profile);
    let database = authority.database.as_ref().expect("real game database");
    println!(
        "database {} v{}: {} disc sets, {} listed games",
        database.path.display(),
        database.version,
        database.disc_sets.len(),
        database.known_serials.len()
    );
    let library = PathBuf::from("/mnt/usbdrive/games/psx");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&library)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "chd"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    let (mut single, mut multi, mut unproven, mut no_serial) = (0, 0, 0, 0);
    let limit: usize = std::env::var("EMUWIZ_PROBE_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120);
    for path in files.iter().take(limit) {
        let report =
            crate::game_identity::inspect_catalogued_game_identity(path, Some("PlayStation"));
        let serial = report.verified_ps1_serial().map(str::to_owned);
        let no_sets: Vec<MediaSet> = Vec::new();
        let dat = TrustedDatAuthority { sets: &no_sets };
        let proof = prove_disc_topology(&query(serial.as_deref(), path), &[&dat, &authority]);
        match &proof {
            DiscTopologyProof::SingleDisc { .. } => single += 1,
            DiscTopologyProof::MultiDisc { .. } => multi += 1,
            DiscTopologyProof::Unproven { missing } if serial.is_none() => {
                no_serial += 1;
                println!("NO SERIAL {}: {missing:?}", path.display());
            }
            _ => {
                unproven += 1;
                println!(
                    "UNPROVEN {} {serial:?}: {}",
                    path.display(),
                    proof.describe()
                );
            }
        }
    }
    println!(
        "sampled {}: single={single} multi={multi} unproven={unproven} no_serial={no_serial}",
        files.len().min(limit)
    );
}
