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

const ROOT_VAR: &str = "EMUWIZ_HINT_GATE_RUNTIME_ROOT";
const CHILD: &str = "rom_organisation_page::hint_gate_runtime_tests::child_both_apply_paths_refuse_a_stale_romm_row";
const VERIFIED_FILE: &str = "v_one.gba";
const VERIFIED_NAME: &str = "Verified One (USA).gba";
const KEPT_FILE: &str = "Kept (USA).gba";
const HINT_FILE: &str = "h_one.gba";

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
        .prefix("emuwiz-hint-gate-runtime-")
        .tempdir()
        .unwrap();
    let root = temp.path().join("profile");
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
        &[(HINT_FILE, &hint)],
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

    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            CHILD,
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ROOT_VAR, &root)
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
}

/// Every game file's bytes, the library database's bytes, and whether a
/// journal folder exists: what an Apply would change.
fn snapshot(root: &Path, journal: &Path) -> Vec<(String, String)> {
    use sha2::{Digest, Sha256};
    let mut out = Vec::new();
    let mut add = |path: &Path| {
        let bytes = std::fs::read(path).unwrap();
        out.push((
            path.display().to_string(),
            Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
        ));
    };
    let mut games: Vec<_> = std::fs::read_dir(root.join("games"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file())
        .collect();
    games.sort();
    games.iter().for_each(|p| add(p));
    add(&root.join("data/library.sqlite3"));
    out.push(("journal exists".into(), journal.exists().to_string()));
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
    assert!(root.join("data/library.sqlite3").is_file() && !root.is_symlink());
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
    assert_ne!(row(&kept).status, OrganisationStatus::Suggested);
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
    let before = snapshot(&root, &journal);

    // Standard Apply: the whole Apply is refused, the verified row included.
    state.apply();
    let error = state.error.clone().expect("Standard Apply must refuse");
    assert!(error.contains(HINT_FILE), "{error}");
    assert!(error.contains("verified catalogue names only"), "{error}");
    assert!(state.applied.is_none(), "nothing was applied");
    assert_eq!(
        snapshot(&root, &journal),
        before,
        "Standard Apply changed something"
    );

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
        snapshot(&root, &journal),
        before,
        "Advanced Protection changed something"
    );
    println!("HINT-GATE-RUNTIME standard=refused advanced=refused unchanged=yes");
}
