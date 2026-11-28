//! Stage B (owner 2026-11-27): the GBC rename preview on a consistent copy of
//! the owner's real library database (existing catalogue checks, RomM
//! metadata and history kept, nothing refreshed first), over copied files.
//! Preview only: nothing is approved or applied.
//!
//! Run only inside a sandbox where /mnt is empty except a bind mount of the
//! copied GBC folder at /mnt/usbdrive/games/gbc, with
//! EMUWIZ_RP_ROOT (scratch profile holding data/ and config/ copies),
//! EMUWIZ_RP_COPY (the copied GBC folder, the bind-mount source) and
//! EMUWIZ_RP_OUT (report folder).
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

use super::*;

fn texts(output: &egui::FullOutput) -> Vec<String> {
    fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
            egui::Shape::Vec(nested) => nested.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

fn render(state: &mut RomOrganisationPageState) -> Vec<String> {
    let ctx = egui::Context::default();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1600.0, 30000.0),
        )),
        ..Default::default()
    };
    let mut output = None;
    for _ in 0..2 {
        output = Some(ctx.run(input.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| show_rom_organisation_page(ui, state));
            });
        }));
    }
    texts(&output.unwrap())
}

#[test]
#[ignore = "real-database GBC preview; run explicitly inside its sandbox"]
fn real_database_gbc_rename_preview() {
    let env = |name: &str| {
        let value = PathBuf::from(std::env::var(name).unwrap_or_else(|_| panic!("{name}")));
        let real = value.canonicalize().unwrap();
        assert!(
            real.starts_with("/tmp/claude-1000/"),
            "{name} is not a scratch path"
        );
        real
    };
    let (root, copy, out) = (
        env("EMUWIZ_RP_ROOT"),
        env("EMUWIZ_RP_COPY"),
        env("EMUWIZ_RP_OUT"),
    );
    assert!(
        archivefs_core::test_environment::isolated_root().is_some(),
        "not an isolated test process"
    );
    // The recorded folder must be either the copy (a bind mount: same device
    // and inode) or a read-only mount of the real folder (the kernel refuses
    // every write, rename and delete there); nothing else is visible under
    // /mnt/usbdrive/games.
    let games = PathBuf::from("/mnt/usbdrive/games/gbc");
    let seen = std::fs::metadata(&games).unwrap();
    let copied = std::fs::metadata(&copy).unwrap();
    let is_copy = (seen.dev(), seen.ino()) == (copied.dev(), copied.ino());
    let read_only = std::fs::read_to_string("/proc/self/mountinfo")
        .unwrap()
        .lines()
        .rfind(|line| line.split(' ').nth(4) == Some("/mnt/usbdrive/games/gbc"))
        .is_some_and(|line| {
            line.split(' ')
                .nth(5)
                .is_some_and(|o| o.split(',').any(|f| f == "ro"))
        });
    assert!(
        is_copy || read_only,
        "neither the copy nor a read-only mount"
    );
    let mut report = String::new();
    use std::fmt::Write as _;
    writeln!(
        report,
        "files: {}",
        if is_copy {
            "copies"
        } else {
            "real folder, read-only mount"
        }
    )
    .unwrap();
    let listed: Vec<_> = std::fs::read_dir("/mnt/usbdrive/games")
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(listed, ["gbc"], "only the copy is visible");
    // SAFETY: this test runs alone (--test-threads=1) before anything reads
    // these variables.
    unsafe {
        std::env::set_var("EMUWIZ_DATA_HOME", root.join("data"));
        std::env::set_var("EMUWIZ_CONFIG_HOME", root.join("config"));
    }
    assert_eq!(
        default_database_path().unwrap(),
        root.join("data/library.sqlite3")
    );
    for path in [
        archivefs_core::default_config_path().unwrap(),
        archivefs_core::dat::sources::default_dat_sources_config_path().unwrap(),
        archivefs_core::identity_source::settings::default_identity_root().unwrap(),
    ] {
        assert!(path.starts_with(&root), "{}", path.display());
    }

    let mut state =
        RomOrganisationPageState::default().with_deferred_source_roots(vec![games.clone()]);
    state.configure_for_test(games.clone(), out.join("journal"));
    state.set_scope(Some(scope::OrganisationScope::Folders(vec![games.clone()])));
    state.generate_plan();
    writeln!(report, "error: {:?}", state.error).unwrap();
    writeln!(
        report,
        "listed sources: {}; not on disk: {}; other filesystems: {}; scan incomplete: {}",
        state.sources.len(),
        state.not_on_disk.len(),
        state.other_filesystems.len(),
        state.source_scan_incomplete.len()
    )
    .unwrap();
    let plan = state.plan.clone().expect("a preview");
    writeln!(report, "rows: {}", plan.entries.len()).unwrap();
    let mut by_status = std::collections::BTreeMap::new();
    let mut by_group = std::collections::BTreeMap::new();
    let mut by_reason = std::collections::BTreeMap::new();
    let mut lines = Vec::new();
    for entry in &plan.entries {
        let group = state.group_of(entry);
        *by_status
            .entry(format!("{:?}", entry.status))
            .or_insert(0_usize) += 1;
        *by_group.entry(group.label()).or_insert(0_usize) += 1;
        if group != scope::RenameGroup::Ready && group != scope::RenameGroup::AlreadyCorrect {
            *by_reason
                .entry(format!(
                    "{} | {}",
                    group.label(),
                    entry.reason.clone().unwrap_or_default()
                ))
                .or_insert(0_usize) += 1;
        }
        let choice = state.name_choices.get(&entry.source_path);
        lines.push(format!(
            "{} | {} -> {} | name source {:?}",
            group.label(),
            entry.source_path.file_name().unwrap().to_string_lossy(),
            entry
                .destination_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            choice.map(|c| &c.source)
        ));
    }
    writeln!(report, "plan statuses: {by_status:?}").unwrap();
    writeln!(report, "groups: {by_group:#?}").unwrap();
    writeln!(report, "not ready, by group and reason: {by_reason:#?}").unwrap();
    let ready_unverified = plan
        .suggested()
        .filter(|e| !hint_gate::name_is_verified(state.name_choices.get(&e.source_path)))
        .count();
    writeln!(
        report,
        "ready rows without a verified name: {ready_unverified}"
    )
    .unwrap();
    lines.sort();
    std::fs::write(out.join("rows.txt"), lines.join("\n")).unwrap();

    // What the page shows: the summary and filter, then each group's rows.
    let mut gui = String::new();
    writeln!(gui, "== page (All) ==").unwrap();
    for text in render(&mut state) {
        writeln!(gui, "{text}").unwrap();
    }
    for group in scope::RenameGroup::ALL {
        state.set_group_filter(Some(group));
        writeln!(gui, "== filter: {} ==", group.label()).unwrap();
        for text in render(&mut state) {
            writeln!(gui, "{text}").unwrap();
        }
    }
    std::fs::write(out.join("gui.txt"), gui).unwrap();
    std::fs::write(out.join("report.txt"), &report).unwrap();
    // Preview only: nothing approved, nothing applied.
    assert!(
        !out.join("journal").exists()
            || std::fs::read_dir(out.join("journal"))
                .unwrap()
                .next()
                .is_none()
    );
    assert_eq!(ready_unverified, 0, "a Ready row without a verified name");
}

/// Owner 2026-11-27: "Reverify the copied GBC files before testing Rename and
/// Undo on those copies." On a fresh copy of the real database (its RomM
/// metadata and history kept), the copied GBC folder is checked again against
/// the No-Intro GBC catalogue and saved, as DAT Management does; then the
/// real preview, Apply of every Ready row and Undo, on the copies only.
/// EMUWIZ_RP_DAT: the registry's catalogue file (read only).
#[test]
#[ignore = "real-database reverify + rename + Undo on copies; run explicitly inside its sandbox"]
fn real_database_gbc_reverify_rename_and_undo() {
    use archivefs_core::dat::rom_organisation::{
        apply_organisation_transaction, bind_organisation_transaction,
        build_organisation_transaction, rollback_organisation_transaction,
    };
    use archivefs_core::dat::sources::DatSourceKind;
    use archivefs_core::dat::sources::audit_cache::AuditCacheConfig;
    use archivefs_core::dat::sources::audit_run::{DatAuditRequest, run_dat_audit_with_cache};
    use sha2::Digest as _;
    use std::collections::{BTreeMap, BTreeSet};
    use std::fmt::Write as _;
    use std::sync::atomic::AtomicBool;
    let env = |name: &str| {
        let value = PathBuf::from(std::env::var(name).unwrap_or_else(|_| panic!("{name}")));
        value.canonicalize().unwrap()
    };
    let (root, copy, out, dat) = (
        env("EMUWIZ_RP_ROOT"),
        env("EMUWIZ_RP_COPY"),
        env("EMUWIZ_RP_OUT"),
        env("EMUWIZ_RP_DAT"),
    );
    for path in [&root, &copy, &out] {
        assert!(
            path.starts_with("/tmp/claude-1000/"),
            "{} is not scratch",
            path.display()
        );
    }
    assert!(archivefs_core::test_environment::isolated_root().is_some());
    // Writes happen here, so the folder must be the copy itself.
    let games = PathBuf::from("/mnt/usbdrive/games/gbc");
    let seen = std::fs::metadata(&games).unwrap();
    let copied = std::fs::metadata(&copy).unwrap();
    assert_eq!(
        (seen.dev(), seen.ino()),
        (copied.dev(), copied.ino()),
        "not the copy"
    );
    let listed: Vec<_> = std::fs::read_dir("/mnt/usbdrive/games")
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(listed, ["gbc"], "only the copy is visible");
    // SAFETY: this test runs alone (--test-threads=1).
    unsafe {
        std::env::set_var("EMUWIZ_DATA_HOME", root.join("data"));
        std::env::set_var("EMUWIZ_CONFIG_HOME", root.join("config"));
    }
    let database_path = default_database_path().unwrap();
    assert_eq!(database_path, root.join("data/library.sqlite3"));

    let hash_all = |dir: &std::path::Path| -> BTreeMap<String, String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| {
                let path = e.unwrap().path();
                let bytes = std::fs::read(&path).unwrap();
                (
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    sha2::Sha256::digest(&bytes)
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>(),
                )
            })
            .collect()
    };
    let before = hash_all(&games);
    let mut report = String::new();

    // 1. Check the copies again and save, as DAT Management does.
    let request = DatAuditRequest {
        source_id: "165".into(),
        source_display_name: "No-Intro: Nintendo - Game Boy Color".into(),
        dat_path: dat,
        dat_kind: DatSourceKind::File,
        scan_root: games.clone(),
        limits: Default::default(),
        policy: None,
        platform: Some("Game Boy Color".into()),
    };
    let outcome = run_dat_audit_with_cache(
        &request,
        &archivefs_core::safe_read::TrustedRoots::from_paths([games.clone()]),
        &AtomicBool::new(false),
        &|_| {},
        AuditCacheConfig::Disabled,
    )
    .unwrap();
    let mut db = archivefs_core::Database::open_or_create(&database_path).unwrap();
    let (saved, _) = db
        .persist_library_dat_identities_and_platforms_from_audit(&outcome, 1)
        .unwrap();
    writeln!(report, "reverified and saved: {saved:?}").unwrap();

    // 2. The real preview.
    let mut state =
        RomOrganisationPageState::default().with_deferred_source_roots(vec![games.clone()]);
    state.configure_for_test(games.clone(), out.join("journal"));
    state.set_scope(Some(scope::OrganisationScope::Folders(vec![games.clone()])));
    state.generate_plan();
    writeln!(report, "preview error: {:?}", state.error).unwrap();
    let plan = state.plan.clone().expect("a preview");
    let mut groups = BTreeMap::new();
    for entry in &plan.entries {
        *groups
            .entry(state.group_of(entry).label())
            .or_insert(0_usize) += 1;
    }
    writeln!(report, "rows: {}; groups: {groups:#?}", plan.entries.len()).unwrap();
    let ready: Vec<_> = plan.suggested().cloned().collect();
    let mut renames: Vec<String> = ready
        .iter()
        .map(|e| {
            format!(
                "{} -> {}",
                e.source_path.file_name().unwrap().to_string_lossy(),
                e.destination_path.file_name().unwrap().to_string_lossy()
            )
        })
        .collect();
    renames.sort();
    std::fs::write(out.join("ready.txt"), renames.join("\n")).unwrap();
    assert!(!ready.is_empty(), "nothing ready: {report}");
    for entry in &ready {
        assert!(hint_gate::name_is_verified(
            state.name_choices.get(&entry.source_path)
        ));
    }

    // 3. Apply every Ready row, then check bytes, records and evidence.
    let approved: BTreeSet<String> = ready
        .iter()
        .map(|e| e.source_path.to_string_lossy().into_owned())
        .collect();
    let ids: Vec<(PathBuf, PathBuf, i64)> = ready
        .iter()
        .map(|e| {
            let id = db
                .find_archive_id_by_absolute_path(&e.source_path)
                .unwrap()
                .unwrap();
            (e.source_path.clone(), e.destination_path.clone(), id)
        })
        .collect();
    let journal = out.join("journal");
    let mut transaction =
        build_organisation_transaction(&plan, &approved, plan.generation).unwrap();
    bind_organisation_transaction(&mut transaction, &db).unwrap();
    apply_organisation_transaction(
        &mut transaction,
        &approved,
        plan.generation,
        archivefs_core::safe_read::TrustedRoots::from_paths([games.clone()]),
        &journal,
        &AtomicBool::new(false),
        archivefs_core::dat::rom_organisation::OrganisationMode::RenameInPlace,
        &games,
        &db,
    )
    .unwrap();
    let after = hash_all(&games);
    assert_eq!(after.len(), before.len(), "no file appears or disappears");
    for (source, destination, id) in &ids {
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
            db.find_archive_id_by_absolute_path(destination).unwrap(),
            Some(*id),
            "{new}"
        );
    }
    let renamed: BTreeSet<String> = ids
        .iter()
        .map(|(s, _, _)| s.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    for (name, hash) in &before {
        if !renamed.contains(name) {
            assert_eq!(after.get(name), Some(hash), "{name}: untouched");
        }
    }
    // The saved checks follow the renamed files: a fresh preview has nothing
    // left to rename among them.
    let mut again =
        RomOrganisationPageState::default().with_deferred_source_roots(vec![games.clone()]);
    again.configure_for_test(games.clone(), out.join("journal-check"));
    again.set_scope(Some(scope::OrganisationScope::Folders(vec![games.clone()])));
    again.generate_plan();
    let after_plan = again.plan.clone().expect("a preview after Apply");
    let still_ready = after_plan.suggested().count();
    let renamed_correct = after_plan
        .entries
        .iter()
        .filter(|e| ids.iter().any(|(_, d, _)| d == &e.source_path))
        .filter(|e| again.group_of(e) == scope::RenameGroup::AlreadyCorrect)
        .count();
    writeln!(
        report,
        "applied: {} renames, same bytes, records follow; preview after Apply: {still_ready} ready, {renamed_correct} of the renamed now already correctly named",
        ids.len()
    )
    .unwrap();
    assert_eq!(still_ready, 0);
    assert_eq!(renamed_correct, ids.len());

    // 4. Undo.
    rollback_organisation_transaction(&mut transaction, &journal, &AtomicBool::new(false), &games)
        .unwrap();
    assert_eq!(
        hash_all(&games),
        before,
        "Undo restores every name and byte"
    );
    for (source, _, id) in &ids {
        assert_eq!(
            db.find_archive_id_by_absolute_path(source).unwrap(),
            Some(*id)
        );
    }
    writeln!(
        report,
        "undo: every name and byte restored ({} files); records on the original paths",
        before.len()
    )
    .unwrap();
    std::fs::write(out.join("report.txt"), &report).unwrap();
}
