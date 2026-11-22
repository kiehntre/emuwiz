//! Runtime proof that both real Apply paths refuse a stale preview whose
//! approved row would be renamed from a RomM title.
//!
//! The parent test builds a throwaway profile with the production scan, DAT
//! verification and persistence (`VerifiedGameFixture`): one game verified
//! under a new catalogue name, one already named correctly, one the catalogue
//! does not list but RomM names. It then runs this test binary again as a
//! child process whose whole environment points into that profile, so the
//! real `NameResolver::load`, preview, Standard Apply and Advanced
//! Protection read only that profile. The child refuses to run anywhere else.

use super::hint_gate::UNVERIFIED_HINT_REASON;
use super::*;
use archivefs_core::verified_game_fixture::{VerifiedGameFixture, synthetic_rom};
use std::collections::BTreeMap;

const ROOT_VAR: &str = "EMUWIZ_HINT_GATE_RUNTIME_ROOT";
const NONCE_VAR: &str = "EMUWIZ_HINT_GATE_RUNTIME_NONCE";
const NONCE_FILE: &str = ".hint-gate-runtime-nonce";
const TEMP_PREFIX: &str = "emuwiz-hint-gate-runtime-";
const CHILD: &str = "rom_organisation_page::hint_gate_runtime_tests::child_both_apply_paths_refuse_a_stale_romm_row";
const VERIFIED_FILE: &str = "v_one.gba";
const VERIFIED_NAME: &str = "Verified One (USA).gba";
const KEPT_FILE: &str = "Kept (USA).gba";
const HINT_FILE: &str = "h_one.gba";
/// No catalogue entry and no RomM title.
const PLAIN_FILE: &str = "n_one.gba";
/// Two files with this name (one in a subfolder): their links would collide.
const CLASH_FILE: &str = "c_one.gba";

fn romm_cache(entries: &[(&Path, &str)]) -> archivefs_core::identity_source::cache::IdentityCache {
    use archivefs_core::identity_source::cache::{CACHE_FORMAT_VERSION, IdentityCache};
    use archivefs_core::identity_source::model::{
        ExternalIdentityRecord, ExternalVerification, IdentityProvider,
    };
    let records = entries
        .iter()
        .enumerate()
        .map(|(i, (path, title))| ExternalIdentityRecord {
            provider: IdentityProvider::Romm,
            server_id: "runtime-test".into(),
            provider_platform_id: None,
            provider_game_id: format!("{i}"),
            provider_file_id: None,
            provider_path: format!("synthetic/{i}"),
            archivefs_path: Some(path.to_path_buf()),
            title: Some((*title).into()),
            platform_candidate: None,
            provider_platform_name: None,
            regions: Vec::new(),
            revision: None,
            hashes: Vec::new(),
            file_size_bytes: None,
            metadata_provider_ids: Vec::new(),
            artwork: None,
            related_files: Vec::new(),
            sibling_game_ids: Vec::new(),
            imported_at_unix_seconds: 1,
            provider_updated_at: None,
            verification: ExternalVerification::StrongExternal,
            conflicts: Vec::new(),
            evidence: Vec::new(),
            synopsis: None,
            genres: Vec::new(),
            players: None,
            rating: None,
            release_year: None,
            howlongtobeat: None,
        })
        .collect::<Vec<_>>();
    IdentityCache {
        format_version: CACHE_FORMAT_VERSION,
        provider: IdentityProvider::Romm,
        server_id: "runtime-test".into(),
        server_version: None,
        source_fingerprint: "runtime-test".into(),
        imported_at_unix_seconds: 1,
        platforms: Vec::new(),
        server_reported_total: Some(records.len() as _),
        records,
        rejected_hashes: Vec::new(),
        unknown_platforms: Vec::new(),
    }
}

#[test]
fn both_apply_paths_refuse_a_stale_romm_row_at_runtime() {
    let temp = tempfile::Builder::new()
        .prefix(TEMP_PREFIX)
        .tempdir()
        .unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap().join("profile");
    let (verified, kept, hint) = (
        synthetic_rom(0x5101),
        synthetic_rom(0x5102),
        synthetic_rom(0x5103),
    );
    let fixture = VerifiedGameFixture::build_at_with_listed_games(
        &root,
        &[
            (VERIFIED_FILE, &verified, VERIFIED_NAME),
            (KEPT_FILE, &kept, KEPT_FILE),
        ],
        &[
            (HINT_FILE, &hint),
            (PLAIN_FILE, &synthetic_rom(0x5104)),
            (CLASH_FILE, &synthetic_rom(0x5105)),
        ],
    )
    .unwrap();
    // A second file with the same name in a subfolder, scanned into the
    // library by the production scan.
    let sub = fixture.games_dir.join("sub");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join(CLASH_FILE), synthetic_rom(0x5106)).unwrap();
    archivefs_core::scan_source_folder_at(
        &fixture.config_path,
        &fixture.database_path,
        &fixture.games_dir,
        "hint-gate-runtime",
    )
    .unwrap();
    // RomM names the unlisted game, and also suggests another title for the
    // verified one (the verified catalogue name must win).
    let games = &fixture.games_dir;
    let cache = romm_cache(&[
        (&games.join(HINT_FILE), "Hint One (USA)"),
        (&games.join(VERIFIED_FILE), "Alpha RomM Guess (USA)"),
    ]);
    let location = archivefs_core::identity_source::cache::IdentityCacheLocation::new(
        &fixture.data_dir.join("identity"),
        archivefs_core::identity_source::model::IdentityProvider::Romm,
    );
    let path = location.cache_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, serde_json::to_vec_pretty(&cache).unwrap()).unwrap();

    // A one-off nonce, written only into this throwaway root: the child
    // refuses any root that does not hold it.
    let nonce = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    std::fs::write(root.join(NONCE_FILE), &nonce).unwrap();

    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            CHILD,
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ROOT_VAR, &root)
        .env(NONCE_VAR, &nonce)
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{text}");
    assert!(
        text.contains("1 passed"),
        "the child test did not run: {text}"
    );
    assert!(
        text.contains("HINT-GATE-RUNTIME standard=refused advanced=refused unchanged=yes"),
        "{text}"
    );
    assert!(
        text.contains("HINT-GATE-RUNTIME linked-library links=4 conflicts=2 rerun=already-present"),
        "{text}"
    );
}

/// Everything under the throwaway root, recursively and without following
/// links: each path with its kind and its content hash (files) or target
/// (links). Database WAL files are included; only SQLite's `-shm` index,
/// which readers update and which holds no data, is left out.
fn snapshot(root: &Path) -> Vec<(String, String)> {
    use sha2::{Digest, Sha256};
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            if path.to_string_lossy().ends_with("-shm") {
                continue;
            }
            let name = path.strip_prefix(root).unwrap().display().to_string();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.file_type().is_symlink() {
                let target = std::fs::read_link(&path).unwrap();
                out.push((name, format!("link -> {}", target.display())));
            } else if meta.is_dir() {
                out.push((name, "dir".into()));
                walk(&path, root, out);
            } else {
                let digest = Sha256::digest(std::fs::read(&path).unwrap());
                let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
                out.push((name, format!("file {hex}")));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

#[test]
#[ignore = "run only by both_apply_paths_refuse_a_stale_romm_row_at_runtime, in its own profile"]
fn child_both_apply_paths_refuse_a_stale_romm_row() {
    let root = PathBuf::from(std::env::var(ROOT_VAR).expect("run only by the parent test"));
    // This child is a test binary, so the crate's pre-main test hook has
    // already given it a private HOME (under `emuwiz-test-env/`) and removed
    // every EMUWIZ_*/XDG override. Point the EmuWiz data and config roots at
    // the throwaway profile, then fail closed unless every path the Apply
    // paths read resolves inside it.
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    assert!(
        home.components()
            .any(|c| c.as_os_str() == "emuwiz-test-env"),
        "not an isolated test process: HOME={}",
        home.display()
    );
    // Only the parent's throwaway root is accepted: canonical (no symlink on
    // the way), directly inside a fresh temp folder named for this test, and
    // holding the parent's nonce. A scratch or real profile can never qualify.
    assert_eq!(
        std::fs::canonicalize(&root).unwrap(),
        root,
        "the root must be canonical"
    );
    let temp_folder = root.parent().unwrap();
    assert_eq!(
        temp_folder.parent(),
        Some(
            std::fs::canonicalize(std::env::temp_dir())
                .unwrap()
                .as_path()
        ),
        "the root must be in this process's temp folder"
    );
    assert!(
        temp_folder
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(TEMP_PREFIX)
    );
    let nonce = std::env::var(NONCE_VAR).expect("run only by the parent test");
    assert_eq!(
        std::fs::read_to_string(root.join(NONCE_FILE)).unwrap(),
        nonce
    );
    for dir in ["data", "config", "games"] {
        let meta = std::fs::symlink_metadata(root.join(dir)).unwrap();
        assert!(meta.is_dir() && !meta.file_type().is_symlink(), "{dir}");
    }
    // SAFETY: this child runs exactly one test (--test-threads=1) and sets
    // these before any code that reads the environment.
    unsafe {
        std::env::set_var("EMUWIZ_DATA_HOME", root.join("data"));
        std::env::set_var("EMUWIZ_CONFIG_HOME", root.join("config"));
    }
    assert_eq!(
        default_database_path().unwrap(),
        root.join("data/library.sqlite3")
    );
    for (what, path) in [
        ("config", archivefs_core::default_config_path().unwrap()),
        (
            "DAT registry",
            archivefs_core::dat::sources::default_dat_sources_config_path().unwrap(),
        ),
        (
            "RomM cache",
            PathBuf::from(default_identity_root().unwrap()),
        ),
    ] {
        assert!(path.starts_with(&root), "{what}: {}", path.display());
    }

    let games = root.join("games");
    let (verified, kept, hint) = (
        games.join(VERIFIED_FILE),
        games.join(KEPT_FILE),
        games.join(HINT_FILE),
    );
    let key = |p: &Path| p.to_string_lossy().into_owned();

    // The real resolver over this profile: a verified catalogue name (beating
    // RomM's other title) and a RomM-only title.
    let names = NameResolver::load();
    let choice = names.choose(&verified);
    assert!(
        matches!(choice.source, NameSource::VerifiedDat { .. }),
        "{choice:?}"
    );
    assert_eq!(choice.name.as_deref(), Some(VERIFIED_NAME));
    assert_eq!(names.choose(&hint).source, NameSource::Romm);

    // The real preview: the verified row is ready, the RomM row is blocked.
    let journal = root.join("data/runtime-journal");
    let mut state =
        RomOrganisationPageState::default().with_deferred_source_roots(vec![games.clone()]);
    state.configure_for_test(games.clone(), journal.clone());
    state.generate_plan();
    assert!(state.error.is_none(), "{:?}", state.error);
    let plan = state.plan.as_ref().expect("a preview");
    let row = |source: &Path| {
        plan.entries
            .iter()
            .find(|e| e.source_path == source)
            .unwrap_or_else(|| panic!("no row for {}", source.display()))
    };
    assert_eq!(row(&verified).status, OrganisationStatus::Suggested);
    assert_eq!(
        row(&verified).destination_path.file_name().unwrap(),
        VERIFIED_NAME
    );
    assert_eq!(row(&hint).status, OrganisationStatus::Blocked);
    assert_eq!(row(&hint).reason.as_deref(), Some(UNVERIFIED_HINT_REASON));
    assert_eq!(
        row(&hint).destination_path.file_name().unwrap(),
        "Hint One (USA).gba",
        "the hint stays visible"
    );
    assert_eq!(row(&kept).status, OrganisationStatus::AlreadyOrganised);
    assert_eq!(row(&kept).reason, None);
    assert_eq!(
        state.approved,
        BTreeSet::from([key(&verified)]),
        "only the verified row is approved"
    );

    // Make the preview stale the way one built before the rule would be: the
    // RomM row is Suggested and approved next to the valid verified row.
    {
        let plan = state.plan.as_mut().unwrap();
        let entry = plan
            .entries
            .iter_mut()
            .find(|e| e.source_path == hint)
            .unwrap();
        entry.status = OrganisationStatus::Suggested;
        entry.reason = None;
        // Re-bound below together, as that preview's own binding pass did.
        for entry in &mut plan.entries {
            entry.catalogue_binding = None;
        }
    }
    state.approved.insert(key(&hint));
    // A preview built before the rule bound every Suggested row to the
    // library catalogue; bind this one the same way (production call).
    {
        let database = Database::open_read_only(default_database_path().unwrap()).unwrap();
        archivefs_core::dat::rom_organisation::prepare_organisation_catalogue(
            state.plan.as_mut().unwrap(),
            &database,
        )
        .unwrap();
    }
    let bound = |plan: &OrganisationPlan, source: &Path| {
        plan.entries
            .iter()
            .find(|e| e.source_path == source)
            .is_some_and(|e| e.catalogue_binding.is_some())
    };
    assert!(
        bound(state.plan.as_ref().unwrap(), &hint),
        "stale row bound"
    );
    assert!(bound(state.plan.as_ref().unwrap(), &verified));
    let before = snapshot(&root);

    // Standard Apply: the whole Apply is refused, the verified row included.
    state.apply();
    let error = state.error.clone().expect("Standard Apply must refuse");
    assert!(error.contains(HINT_FILE), "{error}");
    assert!(error.contains("verified catalogue names only"), "{error}");
    assert!(state.applied.is_none(), "nothing was applied");
    assert_eq!(snapshot(&root), before, "Standard Apply changed something");

    // Advanced Protection, its own validation call (made by its prepare,
    // apply and review steps). Library rows are catalogue-bound, and that
    // path refuses any bound row outright, so the stale selection is refused
    // whole before any name is chosen.
    let plan = state.plan.clone().unwrap();
    let both = crate::gui_v2::validate_selection_for_test(plan.clone(), state.approved.clone())
        .err()
        .expect("Advanced Protection must refuse the whole selection");
    assert!(both.contains("Catalogue-backed"), "{both}");
    let alone =
        crate::gui_v2::validate_selection_for_test(plan.clone(), BTreeSet::from([key(&hint)]))
            .err()
            .expect("Advanced Protection must refuse the stale RomM row");
    assert!(alone.contains("Catalogue-backed"), "{alone}");
    // An unbound stale RomM row (no library binding at all) gets past that
    // rule and is refused by the verified-name check on this path.
    let mut unbound = plan;
    for entry in &mut unbound.entries {
        entry.catalogue_binding = None;
    }
    let named = crate::gui_v2::validate_selection_for_test(unbound, BTreeSet::from([key(&hint)]))
        .err()
        .expect("Advanced Protection must refuse an unbound RomM row");
    assert!(named.contains(HINT_FILE), "{named}");
    assert!(named.contains("verified catalogue names only"), "{named}");
    assert_eq!(
        snapshot(&root),
        before,
        "Advanced Protection changed something"
    );
    println!("HINT-GATE-RUNTIME standard=refused advanced=refused unchanged=yes");

    // Linked library (owner policy 2026-11-22): every eligible game gets a
    // link. A verified catalogue name names it; a RomM-only or unnamed game
    // keeps its own file name; colliding names are Conflicts, never
    // overwritten, suffixed or dropped silently. Game files are untouched.
    let games_before: Vec<_> = snapshot(&root)
        .into_iter()
        .filter(|(path, _)| path.starts_with("games"))
        .collect();
    let library = root.join("linked");
    std::fs::create_dir(&library).unwrap();
    state.set_mode(OrganisationMode::BuildLinkedLibrary);
    state.saved_library_root = Some(library.clone());
    state.generate_plan();
    assert!(state.error.is_none(), "{:?}", state.error);
    let plan = state.plan.clone().expect("a linked-library preview");
    let link_row = |source: &Path| {
        plan.entries
            .iter()
            .find(|e| e.source_path == source)
            .unwrap_or_else(|| panic!("no row for {}", source.display()))
            .clone()
    };
    for (source, name) in [
        (&verified, VERIFIED_NAME),
        (&hint, HINT_FILE),
        (&kept, KEPT_FILE),
        (&games.join(PLAIN_FILE), PLAIN_FILE),
    ] {
        let row = link_row(source);
        assert_eq!(row.status, OrganisationStatus::Suggested, "{row:?}");
        assert_eq!(row.destination_path.file_name().unwrap(), name, "{row:?}");
    }
    let clashes = [games.join(CLASH_FILE), games.join("sub").join(CLASH_FILE)];
    for source in &clashes {
        let row = link_row(source);
        assert_eq!(row.status, OrganisationStatus::Conflict, "{row:?}");
        assert!(row.reason.is_some(), "a conflict says why");
    }
    // Every row is accounted for: four links to create, two conflicts.
    assert_eq!(plan.suggested().count(), 4);
    assert_eq!(
        plan.entries
            .iter()
            .filter(|e| e.status == OrganisationStatus::Conflict)
            .count(),
        2
    );

    state.apply();
    assert!(state.error.is_none(), "{:?}", state.error);
    let mut links = BTreeMap::new();
    fn collect(dir: &Path, out: &mut BTreeMap<String, PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.file_type().is_symlink() {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                assert!(
                    out.insert(name, std::fs::read_link(&path).unwrap())
                        .is_none(),
                    "duplicate link name"
                );
            } else if meta.is_dir() {
                collect(&path, out);
            }
        }
    }
    collect(&library, &mut links);
    assert_eq!(
        links,
        BTreeMap::from([
            (VERIFIED_NAME.to_string(), verified.clone()),
            (HINT_FILE.to_string(), hint.clone()),
            (KEPT_FILE.to_string(), kept.clone()),
            (PLAIN_FILE.to_string(), games.join(PLAIN_FILE)),
        ]),
        "one link per eligible game, nothing for the conflicts"
    );
    let games_after: Vec<_> = snapshot(&root)
        .into_iter()
        .filter(|(path, _)| path.starts_with("games"))
        .collect();
    assert_eq!(games_after, games_before, "game files changed");

    // Preview again: existing links are left as they are.
    state.generate_plan();
    let again = state.plan.clone().unwrap();
    assert_eq!(again.suggested().count(), 0);
    assert_eq!(
        again
            .entries
            .iter()
            .filter(|e| e.status == OrganisationStatus::AlreadyOrganised)
            .count(),
        4
    );
    println!("HINT-GATE-RUNTIME linked-library links=4 conflicts=2 rerun=already-present");
}
