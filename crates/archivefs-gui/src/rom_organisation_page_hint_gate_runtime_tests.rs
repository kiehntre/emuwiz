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
        text.contains("VERIFIED-ROUND-TRIP apply=verified undo=verified"),
        "{text}"
    );
    assert!(
        text.contains("HINT-GATE-RUNTIME link-destination overlap=refused"),
        "{text}"
    );
    assert!(
        text.contains("HINT-GATE-RUNTIME linked-library links=4 conflicts=2 rerun=already-present"),
        "{text}"
    );
    assert!(
        text.contains(
            "HINT-GATE-RUNTIME existing-links moved=4 conflicts=2 refused=4 database=unchanged undo=restored retargeted=refused"
        ),
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
        ("RomM cache", default_identity_root().unwrap()),
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
    // apply and review steps). Catalogue-backed rows are accepted (owner
    // option 1), so the same verified-name check refuses the stale RomM row,
    // alone or with the verified game.
    let plan = state.plan.clone().unwrap();
    let both = crate::gui_v2::validate_selection_for_test(plan.clone(), state.approved.clone())
        .expect_err("Advanced Protection must refuse the whole selection");
    assert!(both.contains(HINT_FILE), "{both}");
    assert!(both.contains("verified catalogue names only"), "{both}");
    let alone =
        crate::gui_v2::validate_selection_for_test(plan.clone(), BTreeSet::from([key(&hint)]))
            .expect_err("Advanced Protection must refuse the stale RomM row");
    assert!(alone.contains("verified catalogue names only"), "{alone}");
    // The verified game alone is eligible, with its sealed library binding.
    let eligible =
        crate::gui_v2::validate_selection_for_test(plan.clone(), BTreeSet::from([key(&verified)]))
            .expect("a current verified game is eligible for Advanced Protection");
    assert!(
        eligible
            .entries
            .iter()
            .all(|e| e.unknown.contains_key("organisation_catalogue_v1")),
        "the library entry follows the file"
    );
    // An unbound stale RomM row (no library binding at all) gets past that
    // rule and is refused by the verified-name check on this path.
    let mut unbound = plan;
    for entry in &mut unbound.entries {
        entry.catalogue_binding = None;
    }
    let named = crate::gui_v2::validate_selection_for_test(unbound, BTreeSet::from([key(&hint)]))
        .expect_err("Advanced Protection must refuse an unbound RomM row");
    assert!(named.contains(HINT_FILE), "{named}");
    assert!(named.contains("verified catalogue names only"), "{named}");
    assert_eq!(
        snapshot(&root),
        before,
        "Advanced Protection changed something"
    );
    println!("HINT-GATE-RUNTIME standard=refused advanced=refused unchanged=yes");

    // A verified game keeps its verified catalogue name across its own
    // Standard Apply and Undo: the saved check moves with the file EmuWiz
    // renamed (a rename changes the file's change time), so the next preview
    // still names it from the catalogue.
    let renamed = games.join(VERIFIED_NAME);
    let verified_choice = |path: &Path| {
        let choice = NameResolver::load().choose(path);
        assert!(
            matches!(choice.source, NameSource::VerifiedDat { .. }),
            "{}: {choice:?}",
            path.display()
        );
        assert_eq!(choice.name.as_deref(), Some(VERIFIED_NAME));
    };
    state.generate_plan();
    assert!(state.error.is_none(), "{:?}", state.error);
    assert_eq!(state.approved, BTreeSet::from([key(&verified)]));
    state.apply();
    assert!(state.error.is_none(), "{:?}", state.error);
    assert!(renamed.is_file() && !verified.exists());
    verified_choice(&renamed);
    // Apply lists the folders again by itself (item 7): no manual rescan.
    assert!(state.sources.contains(&renamed) && !state.sources.contains(&verified));
    state.generate_plan();
    assert!(state.error.is_none(), "{:?}", state.error);
    assert_eq!(
        state
            .plan
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .find(|e| e.source_path == renamed)
            .map(|e| e.status),
        Some(OrganisationStatus::AlreadyOrganised),
        "after Apply the game is still verified and already named: {:?}",
        state
            .plan
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .map(|e| (&e.source_path, e.status))
            .collect::<Vec<_>>()
    );
    state.rollback();
    assert!(state.error.is_none(), "{:?}", state.error);
    assert!(verified.is_file() && !renamed.exists());
    verified_choice(&verified);
    // Undo lists the folders again by itself too.
    assert!(state.sources.contains(&verified) && !state.sources.contains(&renamed));
    state.generate_plan();
    assert!(state.error.is_none(), "{:?}", state.error);
    assert_eq!(state.approved, BTreeSet::from([key(&verified)]));
    println!("VERIFIED-ROUND-TRIP apply=verified undo=verified list=refreshed");

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
    #[allow(clippy::items_after_statements)]
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

    // Reorganise existing links (owner policy 2026-11-22): links made earlier
    // under RomM or other names are named from their target game file - the
    // verified catalogue name when the game is verified, otherwise the game
    // file's own name. Broken and escaping links, and links whose names
    // collide, are refused and left exactly as they are.
    let old_links = root.join("old-links");
    let reorganised = root.join("reorganised");
    std::fs::create_dir(&old_links).unwrap();
    std::fs::create_dir(&reorganised).unwrap();
    let link_to = |name: &str, target: &Path| {
        let link = old_links.join(name);
        std::os::unix::fs::symlink(target, &link).unwrap();
        link
    };
    let plain = games.join(PLAIN_FILE);
    let alpha_link = link_to("Alpha RomM Guess (USA).gba", &verified);
    let romm_link = link_to("Hint One (USA).gba", &hint);
    let plain_link = link_to("Plain Link.gba", &plain);
    let clash_a = link_to("Clash A.gba", &games.join(CLASH_FILE));
    let clash_b = link_to("Clash B.gba", &games.join("sub").join(CLASH_FILE));
    let broken = link_to("Broken.gba", &games.join("gone.gba"));
    // A safe relative link ('..' into the approved games folder) at the same
    // depth as its new folder, so its exact text still reaches the game.
    std::fs::create_dir(old_links.join("sub")).unwrap();
    let relative = old_links.join("sub").join("Relative Kept.gba");
    let relative_text = Path::new("../../games").join(KEPT_FILE);
    std::os::unix::fs::symlink(&relative_text, &relative).unwrap();
    // A relative link one level shallower: moved, its text would reach a
    // different file, so it is refused.
    let shallow = old_links.join("Shallow Relative.gba");
    std::os::unix::fs::symlink(Path::new("..").join("games").join(KEPT_FILE), &shallow).unwrap();
    // A relative link to a file outside the library's approved folders.
    let outside_dir = root.join("outside");
    std::fs::create_dir(&outside_dir).unwrap();
    std::fs::write(outside_dir.join("away.gba"), synthetic_rom(0x5107)).unwrap();
    let outside = old_links.join("Outside.gba");
    std::os::unix::fs::symlink(Path::new("..").join("outside").join("away.gba"), &outside).unwrap();
    // A link to a game file in the approved folder that the library has not
    // indexed: no platform identity, so it is refused, not guessed.
    let unindexed_game = games.join("unindexed.gba");
    std::fs::write(&unindexed_game, synthetic_rom(0x5108)).unwrap();
    let unindexed = link_to("Unindexed.gba", &unindexed_game);
    let links_before = snapshot(&old_links);
    // Every row of every library table, to prove nothing is created,
    // changed or deleted (the moved links have no library rows).
    let database_rows = || {
        let c = rusqlite::Connection::open_with_flags(
            default_database_path().unwrap(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let tables: Vec<String> = c
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let mut rows = Vec::new();
        for table in tables {
            let mut stmt = c.prepare(&format!("SELECT * FROM \"{table}\"")).unwrap();
            let columns = stmt.column_count();
            let mut table_rows: Vec<String> = stmt
                .query_map([], |r| {
                    let mut row = String::new();
                    for i in 0..columns {
                        row.push_str(&format!("{:?}|", r.get_ref(i)?));
                    }
                    Ok(row)
                })
                .unwrap()
                .map(Result::unwrap)
                .collect();
            table_rows.sort();
            rows.push((table, table_rows));
        }
        rows
    };
    let rows_before = database_rows();
    let games_before: Vec<_> = snapshot(&root)
        .into_iter()
        .filter(|(path, _)| path.starts_with("games"))
        .collect();
    let catalogue_paths = || {
        let database =
            archivefs_core::Database::open_read_only(default_database_path().unwrap()).unwrap();
        let mut known = Vec::new();
        for path in [&verified, &hint, &plain, &alpha_link, &romm_link] {
            known.push(
                database
                    .find_archive_id_by_absolute_path(path)
                    .unwrap()
                    .is_some(),
            );
        }
        known
    };
    let catalogue_before = catalogue_paths();
    assert_eq!(catalogue_before, [true, true, true, false, false]);

    let mut links_state =
        RomOrganisationPageState::default().with_deferred_source_roots(vec![old_links.clone()]);
    links_state.configure_for_test(reorganised.clone(), root.join("data/links-journal"));
    links_state.set_mode(OrganisationMode::OrganiseSymlinkOnly);
    // Reorganised links need their own folder (owner decision 2026-11-22):
    // none chosen, the game folder, a folder inside it, or one holding it
    // is refused before anything is planned - never redirected.
    links_state.generate_plan();
    assert!(
        links_state
            .error
            .as_deref()
            .is_some_and(|e| e.contains("choose a separate folder")),
        "{:?}",
        links_state.error
    );
    for (folder, says) in [
        (games.clone(), "is your game folder"),
        (games.join("sub"), "is inside your game folder"),
        (root.clone(), "holds your game folder"),
    ] {
        links_state.saved_library_root = Some(folder.clone());
        links_state.generate_plan();
        let error = links_state
            .error
            .clone()
            .expect("an overlapping folder is refused");
        assert!(error.contains(says), "{}: {error}", folder.display());
        assert!(links_state.plan.is_none());
    }
    println!("HINT-GATE-RUNTIME link-destination overlap=refused");
    links_state.saved_library_root = Some(reorganised.clone());
    links_state.generate_plan();
    assert!(links_state.error.is_none(), "{:?}", links_state.error);
    let plan = links_state.plan.clone().expect("an existing-links preview");
    let row = |link: &Path| {
        plan.entries
            .iter()
            .find(|e| e.source_path == link)
            .unwrap_or_else(|| panic!("no row for {}", link.display()))
            .clone()
    };
    let mut expected = BTreeMap::new();
    for (link, name, target) in [
        (&alpha_link, VERIFIED_NAME, &verified),
        (&romm_link, HINT_FILE, &hint),
        (&plain_link, PLAIN_FILE, &plain),
        (&relative, KEPT_FILE, &kept),
    ] {
        let entry = row(link);
        assert_eq!(entry.status, OrganisationStatus::Suggested, "{entry:?}");
        assert_eq!(
            entry.destination_path.file_name().unwrap(),
            name,
            "{entry:?}"
        );
        assert!(entry.destination_path.starts_with(&reorganised));
        let text = if link == &relative {
            relative_text.clone()
        } else {
            target.clone()
        };
        expected.insert(name.to_string(), text);
        // Each link inherits its game file's platform.
        assert_eq!(
            entry.platform.as_deref(),
            Some("Game Boy Advance"),
            "{entry:?}"
        );
    }
    for link in [&clash_a, &clash_b] {
        let entry = row(link);
        assert_eq!(entry.status, OrganisationStatus::Conflict, "{entry:?}");
        assert!(entry.reason.is_some());
    }
    for (link, words) in [
        (&broken, "broken"),
        (&outside, "outside the library's approved folders"),
        (&unindexed, "no platform identity"),
        (&shallow, "different file"),
    ] {
        let entry = row(link);
        assert_eq!(entry.status, OrganisationStatus::Blocked, "{entry:?}");
        assert!(
            entry.reason.as_deref().unwrap().contains(words),
            "{entry:?}"
        );
    }
    assert_eq!(plan.entries.len(), 10, "every link is accounted for");
    assert_eq!(plan.suggested().count(), 4);

    links_state.apply();
    assert!(links_state.error.is_none(), "{:?}", links_state.error);
    let mut moved = BTreeMap::new();
    collect(&reorganised, &mut moved);
    assert_eq!(
        moved, expected,
        "each link under its game's name, exact target kept"
    );
    for link in [&alpha_link, &romm_link, &plain_link, &relative] {
        assert!(
            std::fs::symlink_metadata(link).is_err(),
            "{} still there",
            link.display()
        );
    }
    // Refused links are exactly as they were.
    let left: Vec<_> = snapshot(&old_links);
    let refused: Vec<_> = links_before
        .iter()
        .filter(|(name, _)| {
            [
                "Clash A.gba",
                "Clash B.gba",
                "Broken.gba",
                "Outside.gba",
                "Unindexed.gba",
                "Shallow Relative.gba",
                "sub",
            ]
            .contains(&name.as_str())
        })
        .cloned()
        .collect();
    assert_eq!(left, refused);
    let games_now: Vec<_> = snapshot(&root)
        .into_iter()
        .filter(|(path, _)| path.starts_with("games"))
        .collect();
    assert_eq!(games_now, games_before, "game files changed");
    assert_eq!(
        catalogue_paths(),
        catalogue_before,
        "the library's game rows did not move"
    );
    assert!(
        database_rows() == rows_before,
        "the library database changed on Apply"
    );

    // Undo puts every link back under its old name with its exact target.
    links_state.rollback();
    assert!(links_state.error.is_none(), "{:?}", links_state.error);
    assert_eq!(
        snapshot(&old_links),
        links_before,
        "Undo restored the links"
    );
    let mut after_undo = BTreeMap::new();
    collect(&reorganised, &mut after_undo);
    assert!(after_undo.is_empty(), "{after_undo:?}");
    let games_now: Vec<_> = snapshot(&root)
        .into_iter()
        .filter(|(path, _)| path.starts_with("games"))
        .collect();
    assert_eq!(games_now, games_before, "game files changed by Undo");
    assert_eq!(catalogue_paths(), catalogue_before);
    assert!(
        database_rows() == rows_before,
        "the library database changed on Undo"
    );

    // A link pointed at another game after the preview is not moved.
    links_state.generate_plan();
    assert!(links_state.error.is_none(), "{:?}", links_state.error);
    std::fs::remove_file(&romm_link).unwrap();
    std::os::unix::fs::symlink(&plain, &romm_link).unwrap();
    let before_retarget = snapshot(&root);
    links_state.apply();
    assert!(
        links_state.error.is_some(),
        "a retargeted link must refuse the Apply"
    );
    assert!(links_state.applied.is_none());
    assert_eq!(snapshot(&root), before_retarget, "nothing changed");
    println!(
        "HINT-GATE-RUNTIME existing-links moved=4 conflicts=2 refused=4 database=unchanged undo=restored retargeted=refused"
    );
}
