//! The shared ZIP naming rule (owner decision 2026-11-26), end to end on the
//! real route: real archives, a real catalogue check and save, the saved
//! verified names, `choose_name`, the evidence-first plan, Apply and Undo.
//! Only an archive holding one exactly verified game (a single file or a
//! complete multi-file set) takes the game's name; contents never change.
use std::collections::{BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use archivefs_core::Database;
use archivefs_core::dat::configured_catalogues::ConfiguredCatalogue;
use archivefs_core::dat::rom_organisation::{
    OrganisationCandidate, OrganisationMode, OrganisationPlan, OrganisationPlanRequest,
    OrganisationStatus, apply_organisation_transaction, bind_organisation_transaction,
    build_organisation_transaction, build_rename_plan_from_evidence,
    prepare_organisation_catalogue, rollback_organisation_transaction,
};
use archivefs_core::dat::sources::DatSourceKind;
use archivefs_core::dat::sources::audit_cache::AuditCacheConfig;
use archivefs_core::dat::sources::audit_run::{DatAuditRequest, run_dat_audit_with_cache};
use archivefs_core::job_progress::ProgressReporter;
use archivefs_core::platform::identity::resolve_platform_identity;
use sha1::Digest;

use super::{ARCHIVE_RENAME_NOTE, NameSource, choose_name, hint_gate};

const GBC: &str = "Game Boy Color";
const HEADER: &str = "Nintendo - Game Boy Color";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha1(bytes: &[u8]) -> String {
    hex(&sha1::Sha1::digest(bytes))
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    hex(&sha2::Sha256::digest(bytes))
}

fn zip(path: &Path, members: &[(&str, &[u8])]) {
    let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, body) in members {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(body).unwrap();
    }
    writer.finish().unwrap();
}

struct Library {
    _dir: tempfile::TempDir,
    root: PathBuf,
    db: Database,
    ids: HashMap<PathBuf, i64>,
    catalogues: HashMap<String, ConfiguredCatalogue>,
}

/// The catalogue: one-file games, a two-file game, and a two-file game
/// whose archive holds only one of its files.
const GAMES: &[(&str, &[(&str, &[u8])])] = &[
    ("Alpha (USA)", &[("Alpha (USA).gbc", b"alpha rom")]),
    ("Upper (Europe)", &[("Upper (Europe).gbc", b"upper rom")]),
    (
        "Beta (USA)",
        &[
            ("Beta (USA) (Track 1).gbc", b"beta one"),
            ("Beta (USA) (Track 2).gbc", b"beta two"),
        ],
    ),
    ("Gamma (USA)", &[("Gamma (USA).gbc", b"gamma rom")]),
    ("Delta (USA)", &[("Delta (USA).gbc", b"delta rom")]),
    (
        "Epsilon (USA)",
        &[
            ("Epsilon (USA) (Part 1).gbc", b"epsilon one"),
            ("Epsilon (USA) (Part 2).gbc", b"epsilon two"),
        ],
    ),
    ("Zeta (USA)", &[("Zeta (USA).gbc", b"zeta rom")]),
];

fn library() -> Library {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("gbc");
    std::fs::create_dir_all(&root).unwrap();
    zip(&root.join("alpha.zip"), &[("alpha.gbc", b"alpha rom")]);
    zip(&root.join("upper.ZIP"), &[("upper.gbc", b"upper rom")]);
    zip(
        &root.join("beta.zip"),
        &[("b1.gbc", b"beta one"), ("b2.gbc", b"beta two")],
    );
    zip(
        &root.join("two games.zip"),
        &[("g.gbc", b"gamma rom"), ("d.gbc", b"delta rom")],
    );
    zip(&root.join("partial.zip"), &[("e1.gbc", b"epsilon one")]);
    zip(&root.join("zeta.zip"), &[("z.gbc", b"zeta rom")]);
    // An unrelated file already holds the name zeta.zip would take.
    std::fs::write(root.join("Zeta (USA).zip"), b"someone else's file").unwrap();
    // A loose verified file, and an unknown file a RomM hint would call by
    // the same game's name (the RomM conflict test).
    std::fs::write(root.join("gamma.gbc"), b"gamma rom").unwrap();
    std::fs::write(root.join("hinted.gbc"), b"not in the catalogue").unwrap();

    let dat = dir.path().join("gbc.dat");
    let mut body = format!(
        "<datafile><header><name>{HEADER}</name><description>{HEADER}</description><version>20260101</version><author>No-Intro</author></header>"
    );
    for (game, roms) in GAMES {
        body.push_str(&format!("<game name=\"{game}\">"));
        for (rom, bytes) in *roms {
            body.push_str(&format!(
                "<rom name=\"{rom}\" size=\"{}\" sha1=\"{}\"/>",
                bytes.len(),
                sha1(bytes)
            ));
        }
        body.push_str("</game>");
    }
    body.push_str("</datafile>");
    std::fs::write(&dat, &body).unwrap();

    let mut db = Database::open_or_create(dir.path().join("library.sqlite3")).unwrap();
    db.register_source_folders(std::slice::from_ref(&root))
        .unwrap();
    let folder = db.list_source_folders().unwrap()[0].id;
    let mut ids = HashMap::new();
    let mut names: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    names.sort();
    for path in names {
        let archive = archivefs_core::Archive::from_path_in_root(&path, &root).unwrap();
        let id = db
            .upsert_archive(folder, &root, &archive)
            .unwrap()
            .archive_id;
        db.set_manual_platform(id, GBC).unwrap();
        ids.insert(path, id);
    }

    let request = DatAuditRequest {
        source_id: "gbc".into(),
        source_display_name: "No-Intro GBC".into(),
        dat_path: dat.clone(),
        dat_kind: DatSourceKind::File,
        scan_root: root.clone(),
        limits: Default::default(),
        policy: None,
        platform: Some(GBC.into()),
    };
    let outcome = run_dat_audit_with_cache(
        &request,
        &archivefs_core::safe_read::TrustedRoots::from_paths([root.clone()]),
        &AtomicBool::new(false),
        &|_| {},
        AuditCacheConfig::Disabled,
    )
    .unwrap();
    db.persist_library_dat_identities_and_platforms_from_audit(&outcome, 1)
        .unwrap();
    let catalogues = HashMap::from([(
        "gbc".to_string(),
        ConfiguredCatalogue {
            reference: archivefs_core::dat::catalogue_selection::CatalogueRef::local("gbc"),
            source_id: "gbc".into(),
            display_name: "No-Intro GBC".into(),
            path: dat,
            kind: DatSourceKind::File,
            sha256: Some(sha256(body.as_bytes())),
            revision: Some("20260101".into()),
        },
    )]);
    Library {
        _dir: dir,
        root,
        db,
        ids,
        catalogues,
    }
}

struct Proposal {
    candidates: Vec<OrganisationCandidate>,
    raw_sha1: HashMap<PathBuf, String>,
    choices: HashMap<PathBuf, super::NameChoice>,
}

/// What bulk Rename proposes from the saved checks, as `evidence_candidates` does.
fn propose(lib: &Library) -> Proposal {
    propose_with_romm(lib, &HashMap::new())
}

/// As [`propose`], with RomM titles for some files (display hints only).
fn propose_with_romm(lib: &Library, romm: &HashMap<PathBuf, String>) -> Proposal {
    let ids: Vec<i64> = lib.ids.values().copied().collect();
    let evidence = lib
        .db
        .verified_dat_names_for_archives(&ids, &lib.catalogues)
        .unwrap();
    let mut out = Proposal {
        candidates: Vec::new(),
        raw_sha1: HashMap::new(),
        choices: HashMap::new(),
    };
    for (path, id) in &lib.ids {
        let found = evidence.get(id);
        let dat = found
            .map(|e| e.name.clone())
            .unwrap_or(archivefs_core::VerifiedDatName::None);
        let choice = choose_name(dat, romm.get(path).cloned(), path);
        if let (Some(found), NameSource::VerifiedDat { .. }) = (found, &choice.source)
            && let Some(sha1) = &found.raw_sha1
        {
            out.raw_sha1.insert(path.clone(), sha1.clone());
        }
        out.candidates.push(OrganisationCandidate {
            source_path: path.clone(),
            resolution: resolve_platform_identity(
                1,
                lib.db.current_platform_identity_evidence(*id, 1).unwrap(),
            ),
            canonical_name: hint_gate::bulk_name(OrganisationMode::RenameInPlace, &choice),
            content_classification: None,
            original_metadata: Default::default(),
        });
        out.choices.insert(path.clone(), choice);
    }
    out
}

fn plan(lib: &Library, proposal: &Proposal) -> OrganisationPlan {
    let mut ignore = |_: archivefs_core::job_progress::JobProgress| {};
    let mut progress = ProgressReporter::new(&mut ignore);
    let mut plan = build_rename_plan_from_evidence(
        &OrganisationPlanRequest {
            master_root: &lib.root,
            mode: OrganisationMode::RenameInPlace,
            content_policy: archivefs_core::dat::classification::ContentSelectionPolicy::AllEntries,
            candidates: &proposal.candidates,
            generation: 1,
            unknown_folder: None,
            link_target_roots: None,
        },
        &proposal.raw_sha1,
        None,
        &mut progress,
    )
    .unwrap();
    prepare_organisation_catalogue(&mut plan, &lib.db).unwrap();
    plan
}

fn entry<'a>(
    plan: &'a OrganisationPlan,
    name: &str,
) -> &'a archivefs_core::dat::rom_organisation::OrganisationPlanEntry {
    plan.entries
        .iter()
        .find(|e| e.source_path.file_name().unwrap() == name)
        .unwrap_or_else(|| panic!("{name} not planned"))
}

fn files(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| {
            let path = e.unwrap().path();
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&path).unwrap(),
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn only_an_archive_holding_one_verified_game_takes_its_name_and_undo_restores_it() {
    let lib = library();
    let proposal = propose(&lib);
    let chosen = |name: &str| proposal.choices[&lib.root.join(name)].clone();

    // A one-file game and a complete two-file game take the game's name,
    // keeping their own extension as written.
    for (file, expected) in [
        ("alpha.zip", "Alpha (USA).zip"),
        ("upper.ZIP", "Upper (Europe).ZIP"),
        ("beta.zip", "Beta (USA).zip"),
        ("zeta.zip", "Zeta (USA).zip"),
    ] {
        let choice = chosen(file);
        assert_eq!(choice.name.as_deref(), Some(expected), "{file}: {choice:?}");
        assert!(matches!(choice.source, NameSource::VerifiedDat { .. }));
        assert_eq!(choice.note.as_deref(), Some(ARCHIVE_RENAME_NOTE));
    }
    // Two games in one archive, or part of a game: never a name.
    for file in ["two games.zip", "partial.zip"] {
        let choice = chosen(file);
        assert_eq!(choice.name, None, "{file}: {choice:?}");
        assert_eq!(choice.source, NameSource::Original, "{file}");
    }

    let plan = plan(&lib, &proposal);
    let status = |name: &str| entry(&plan, name).status;
    for file in ["alpha.zip", "upper.ZIP", "beta.zip"] {
        assert_eq!(
            status(file),
            OrganisationStatus::Suggested,
            "{file}: {:?}",
            entry(&plan, file).reason
        );
    }
    // A real collision is still a conflict.
    assert_eq!(status("zeta.zip"), OrganisationStatus::Conflict);
    for file in ["two games.zip", "partial.zip"] {
        assert_ne!(status(file), OrganisationStatus::Suggested, "{file}");
    }

    // Apply the three, then Undo.
    let before = files(&lib.root);
    let approved: BTreeSet<String> = ["alpha.zip", "upper.ZIP", "beta.zip"]
        .iter()
        .map(|f| lib.root.join(f).to_string_lossy().into_owned())
        .collect();
    let journal = lib.root.parent().unwrap().join("journal");
    let mut transaction = build_organisation_transaction(&plan, &approved, 1).unwrap();
    bind_organisation_transaction(&mut transaction, &lib.db).unwrap();
    apply_organisation_transaction(
        &mut transaction,
        &approved,
        1,
        archivefs_core::safe_read::TrustedRoots::from_paths([lib.root.clone()]),
        &journal,
        &AtomicBool::new(false),
        OrganisationMode::RenameInPlace,
        &lib.root,
        &lib.db,
    )
    .unwrap();
    // Same bytes under the new names: nothing inside an archive changed.
    let renamed = |name: &str| -> Vec<u8> { std::fs::read(lib.root.join(name)).unwrap() };
    let original =
        |name: &str| -> Vec<u8> { before.iter().find(|(n, _)| n == name).unwrap().1.clone() };
    assert_eq!(renamed("Alpha (USA).zip"), original("alpha.zip"));
    assert_eq!(renamed("Upper (Europe).ZIP"), original("upper.ZIP"));
    assert_eq!(renamed("Beta (USA).zip"), original("beta.zip"));
    let mut members: Vec<String> =
        zip::ZipArchive::new(std::fs::File::open(lib.root.join("Beta (USA).zip")).unwrap())
            .unwrap()
            .file_names()
            .map(|name| name.unwrap().to_string())
            .collect();
    members.sort();
    assert_eq!(members, ["b1.gbc", "b2.gbc"], "members keep their names");
    // The verified evidence follows the file: same id, still the same name.
    let alpha = lib.ids[&lib.root.join("alpha.zip")];
    assert_eq!(
        lib.db
            .find_archive_id_by_absolute_path(&lib.root.join("Alpha (USA).zip"))
            .unwrap(),
        Some(alpha)
    );
    match lib
        .db
        .verified_dat_name_for_archive(alpha, &lib.catalogues)
        .unwrap()
    {
        archivefs_core::VerifiedDatName::Usable { game_name, .. } => {
            assert_eq!(game_name, "Alpha (USA)")
        }
        other => panic!("evidence lost after rename: {other:?}"),
    }
    // Untouched: the refused archives and the colliding file.
    for name in ["two games.zip", "partial.zip", "zeta.zip", "Zeta (USA).zip"] {
        assert_eq!(renamed(name), original(name), "{name}");
    }

    rollback_organisation_transaction(
        &mut transaction,
        &journal,
        &AtomicBool::new(false),
        &lib.root,
    )
    .unwrap();
    assert_eq!(
        files(&lib.root),
        before,
        "Undo restores every name and byte"
    );
    assert_eq!(
        lib.db
            .find_archive_id_by_absolute_path(&lib.root.join("alpha.zip"))
            .unwrap(),
        Some(alpha)
    );
}

/// 7z and RAR archives keep the old protection: never renamed by a member's
/// match. RomM never names an archive.
#[test]
fn other_containers_and_romm_never_name_an_archive() {
    use archivefs_core::VerifiedDatName as V;
    let member = || V::Usable {
        rom_name: "Lemmings (1991).adf".into(),
        game_name: "Lemmings (1991)".into(),
        dat_source_id: "tosec-amiga".into(),
        through_archive_member: true,
    };
    let seven = choose_name(member(), None, Path::new("/roms/amiga/lemmings.7z"));
    assert_eq!((seven.name, seven.source), (None, NameSource::Original));
    assert!(seven.skipped_archive);
    // RomM alone is a hint: not a verified name, so bulk Rename blocks the
    // row (the shared gate) rather than renaming the archive with it.
    let romm = choose_name(V::None, Some("Lemmings.zip".into()), Path::new("/r/l.zip"));
    assert_eq!(romm.source, NameSource::Romm);
    assert!(!hint_gate::name_is_verified(Some(&romm)), "{romm:?}");
    // A verified ZIP wins over a RomM title.
    let zip = choose_name(member(), Some("Other.zip".into()), Path::new("/r/l.zip"));
    assert_eq!(zip.name.as_deref(), Some("Lemmings (1991).zip"));
}

/// Owner 2026-11-26: "Unverified RomM metadata must not become a proposed
/// actionable filename or create misleading destination conflicts in
/// verified Rename mode." A RomM hint naming an unknown file after a verified
/// game proposes nothing, and the verified file still renames.
#[test]
fn a_romm_hint_never_takes_a_name_or_blocks_a_verified_rename() {
    let lib = library();
    let hinted = lib.root.join("hinted.gbc");
    let romm = HashMap::from([(hinted.clone(), "Gamma (USA)".to_string())]);
    let proposal = propose_with_romm(&lib, &romm);
    let choice = &proposal.choices[&hinted];
    assert_eq!(choice.source, NameSource::Romm, "{choice:?}");
    assert!(!hint_gate::name_is_verified(Some(choice)));
    let candidate = proposal
        .candidates
        .iter()
        .find(|c| c.source_path == hinted)
        .unwrap();
    assert_eq!(
        candidate.canonical_name, None,
        "a hint is never an actionable name"
    );

    let plan = plan(&lib, &proposal);
    let gamma = entry(&plan, "gamma.gbc");
    assert_eq!(
        gamma.status,
        OrganisationStatus::Suggested,
        "{:?}",
        gamma.reason
    );
    assert_eq!(gamma.destination_path, lib.root.join("Gamma (USA).gbc"));
    let hint = entry(&plan, "hinted.gbc");
    assert_ne!(hint.status, OrganisationStatus::Suggested);
    assert_ne!(
        hint.status,
        OrganisationStatus::Conflict,
        "{:?}",
        hint.reason
    );
    assert_ne!(hint.destination_path, lib.root.join("Gamma (USA).gbc"));
}

/// Stage B B6 (owner 2026-11-26): "Verify the 122 eligible GBC renames on
/// copied data, including Undo. Do not perform production renames."
/// Runs only when pointed at a scratch copy (never the source collection):
/// EMUWIZ_B6_ROOT (copied GBC folder), EMUWIZ_B6_DAT (copied No-Intro GBC DAT),
/// EMUWIZ_B6_OUT (report folder). Real route: register, catalogue check and
/// save, saved verified names, choose_name, the evidence-first plan, Apply of
/// every Ready row, byte and evidence checks, then Undo of all of it.
#[test]
#[ignore = "B6 rehearsal on copied data; run explicitly with EMUWIZ_B6_* set"]
fn b6_rehearsal_on_copied_gbc_files() {
    use std::fmt::Write as _;
    let root = PathBuf::from(std::env::var("EMUWIZ_B6_ROOT").expect("EMUWIZ_B6_ROOT"));
    let dat = PathBuf::from(std::env::var("EMUWIZ_B6_DAT").expect("EMUWIZ_B6_DAT"));
    let out = PathBuf::from(std::env::var("EMUWIZ_B6_OUT").expect("EMUWIZ_B6_OUT"));
    for path in [&root, &dat, &out] {
        let real = path.canonicalize().unwrap();
        assert!(
            real.starts_with("/tmp/claude-1000/") && !real.starts_with("/mnt"),
            "B6 runs only on a scratch copy: {}",
            real.display()
        );
    }
    let hash_all = |root: &Path| -> std::collections::BTreeMap<String, String> {
        std::fs::read_dir(root)
            .unwrap()
            .map(|e| {
                let path = e.unwrap().path();
                (
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    sha256(&std::fs::read(&path).unwrap()),
                )
            })
            .collect()
    };
    let before = hash_all(&root);
    let body = std::fs::read(&dat).unwrap();
    let db_dir = tempfile::tempdir_in(&out).unwrap();
    let mut db = Database::open_or_create(db_dir.path().join("library.sqlite3")).unwrap();
    db.register_source_folders(std::slice::from_ref(&root))
        .unwrap();
    let folder = db.list_source_folders().unwrap()[0].id;
    let mut ids = HashMap::new();
    for name in before.keys() {
        let path = root.join(name);
        let archive = archivefs_core::Archive::from_path_in_root(&path, &root).unwrap();
        let id = db
            .upsert_archive(folder, &root, &archive)
            .unwrap()
            .archive_id;
        db.set_manual_platform(id, GBC).unwrap();
        ids.insert(path, id);
    }
    let request = DatAuditRequest {
        source_id: "165".into(),
        source_display_name: "No-Intro: Nintendo - Game Boy Color".into(),
        dat_path: dat.clone(),
        dat_kind: DatSourceKind::File,
        scan_root: root.clone(),
        limits: Default::default(),
        policy: None,
        platform: Some(GBC.into()),
    };
    let outcome = run_dat_audit_with_cache(
        &request,
        &archivefs_core::safe_read::TrustedRoots::from_paths([root.clone()]),
        &AtomicBool::new(false),
        &|_| {},
        AuditCacheConfig::Disabled,
    )
    .unwrap();
    db.persist_library_dat_identities_and_platforms_from_audit(&outcome, 1)
        .unwrap();
    let catalogues = HashMap::from([(
        "165".to_string(),
        ConfiguredCatalogue {
            reference: archivefs_core::dat::catalogue_selection::CatalogueRef::local("165"),
            source_id: "165".into(),
            display_name: "No-Intro: Nintendo - Game Boy Color".into(),
            path: dat.clone(),
            kind: DatSourceKind::File,
            sha256: Some(sha256(&body)),
            revision: None,
        },
    )]);
    let lib = Library {
        _dir: tempfile::tempdir().unwrap(),
        root: root.clone(),
        db,
        ids,
        catalogues,
    };
    let proposal = propose(&lib);
    let plan = plan(&lib, &proposal);

    let mut report = String::new();
    let mut by_status = std::collections::BTreeMap::new();
    for entry in &plan.entries {
        *by_status
            .entry(format!("{:?}", entry.status))
            .or_insert(0_usize) += 1;
    }
    writeln!(report, "files: {}", before.len()).unwrap();
    writeln!(report, "plan statuses: {by_status:?}").unwrap();
    let mut reasons = std::collections::BTreeMap::new();
    for entry in plan.entries.iter().filter(|e| {
        e.status != OrganisationStatus::Suggested
            && e.status != OrganisationStatus::AlreadyOrganised
    }) {
        *reasons
            .entry(entry.reason.clone().unwrap_or_default())
            .or_insert(0_usize) += 1;
    }
    writeln!(report, "not-ready reasons: {reasons:#?}").unwrap();
    let ready: Vec<_> = plan.suggested().collect();
    for entry in &ready {
        // Every proposal is a verified catalogue name and a same-folder rename.
        let choice = &proposal.choices[&entry.source_path];
        assert!(
            matches!(choice.source, NameSource::VerifiedDat { .. }),
            "{choice:?}"
        );
        assert_eq!(entry.destination_path.parent(), entry.source_path.parent());
        assert!(
            !entry.destination_path.exists()
                || entry.destination_path.to_string_lossy().to_lowercase()
                    == entry.source_path.to_string_lossy().to_lowercase()
        );
        writeln!(
            report,
            "RENAME {} -> {}",
            entry.source_path.file_name().unwrap().to_string_lossy(),
            entry
                .destination_path
                .file_name()
                .unwrap()
                .to_string_lossy()
        )
        .unwrap();
    }
    writeln!(report, "ready: {}", ready.len()).unwrap();
    std::fs::write(out.join("b6-preview.txt"), &report).unwrap();
    assert!(!ready.is_empty(), "nothing ready: see b6-preview.txt");

    let approved: BTreeSet<String> = ready
        .iter()
        .map(|e| e.source_path.to_string_lossy().into_owned())
        .collect();
    let moves: Vec<(PathBuf, PathBuf, i64)> = ready
        .iter()
        .map(|e| {
            (
                e.source_path.clone(),
                e.destination_path.clone(),
                lib.ids[&e.source_path],
            )
        })
        .collect();
    let journal = out.join("journal");
    let mut transaction = build_organisation_transaction(&plan, &approved, 1).unwrap();
    bind_organisation_transaction(&mut transaction, &lib.db).unwrap();
    apply_organisation_transaction(
        &mut transaction,
        &approved,
        1,
        archivefs_core::safe_read::TrustedRoots::from_paths([root.clone()]),
        &journal,
        &AtomicBool::new(false),
        OrganisationMode::RenameInPlace,
        &root,
        &lib.db,
    )
    .unwrap();
    let after = hash_all(&root);
    assert_eq!(after.len(), before.len(), "no file appears or disappears");
    for (source, destination, id) in &moves {
        let old = source.file_name().unwrap().to_string_lossy().into_owned();
        let new = destination
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            after.get(&new),
            before.get(&old),
            "{old} -> {new}: same bytes"
        );
        assert_eq!(
            lib.db
                .find_archive_id_by_absolute_path(destination)
                .unwrap(),
            Some(*id),
            "{new}: the library record follows the file"
        );
        assert!(
            matches!(
                lib.db
                    .verified_dat_name_for_archive(*id, &lib.catalogues)
                    .unwrap(),
                archivefs_core::VerifiedDatName::Usable { .. }
            ),
            "{new}: verified evidence kept"
        );
    }
    let renamed: BTreeSet<String> = moves
        .iter()
        .map(|(s, _, _)| s.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    for (name, hash) in &before {
        if !renamed.contains(name) {
            assert_eq!(after.get(name), Some(hash), "{name}: untouched");
        }
    }
    let applied = format!(
        "applied: {} renames; bytes identical; records and evidence follow\n",
        moves.len()
    );

    rollback_organisation_transaction(&mut transaction, &journal, &AtomicBool::new(false), &root)
        .unwrap();
    assert_eq!(hash_all(&root), before, "Undo restores every name and byte");
    for (source, _, id) in &moves {
        assert_eq!(
            lib.db.find_archive_id_by_absolute_path(source).unwrap(),
            Some(*id)
        );
    }
    std::fs::write(
        out.join("b6-result.txt"),
        format!("{applied}undo: every name and byte restored ({} files); records back on the original paths\n", before.len()),
    )
    .unwrap();
}
