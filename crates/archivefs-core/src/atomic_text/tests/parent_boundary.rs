//! Deterministic parent-rebinding reproductions, using synthetic files only.

use super::{Event, fixture, install_hook, snapshot, stages, write};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Path, PathBuf};

fn object(path: &Path) -> Value {
    match fs::symlink_metadata(path) {
        Ok(meta) => json!({
            "dev": meta.dev(), "ino": meta.ino(), "links": meta.nlink(),
            "mode": meta.mode(),
            "bytes": if meta.is_file() { Some(fs::read(path).unwrap()) } else { None },
            "symlink": fs::read_link(path).ok(),
            "resolved_directory": fs::metadata(path).ok().filter(|m| m.is_dir())
                .map(|m| (m.dev(), m.ino())),
        }),
        Err(error) => json!({"error": error.to_string()}),
    }
}

fn tree(path: &Path) -> Value {
    let mut entries = fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    entries.sort();
    json!({"directory": object(path), "entries": entries.into_iter()
        .map(|path| json!({"name": path.file_name(), "object": object(&path)})).collect::<Vec<_>>()})
}

fn evidence(case: &str, before: Value, after: Value, result: &impl std::fmt::Debug) {
    println!(
        "PARENT_EVIDENCE {}",
        json!({"case": case, "before": before,
        "after": after, "result": format!("{result:?}")})
    );
}

/// A replacement directory contains a foreign stage at the same basename.
/// It must survive, while the owned stage in the original directory is cleaned.
#[test]
fn replaced_parent_refuses_and_cleans_original_stage() {
    let root = fixture();
    let parent = root.path().join("parent");
    let parked = root.path().join("original-directory");
    fs::create_dir(&parent).unwrap();
    let target = parent.join("destination");
    fs::write(&target, "original destination").unwrap();
    let original = snapshot(&target);
    let before = tree(&parent);
    let (p, a) = (parent.clone(), parked.clone());
    let _reset = install_hook(move |event, stage| {
        if event == Event::Created {
            fs::rename(&p, &a)?;
            fs::create_dir(&p)?;
            fs::write(p.join("destination"), "replacement destination")?;
            fs::write(p.join(stage.file_name().unwrap()), "other operation stage")?;
        }
        Ok(())
    });
    let result = write(&target, "new text");
    evidence(
        "parent-replaced",
        before,
        json!({"original":tree(&parked),"replacement":tree(&parent)}),
        &result,
    );
    assert!(result.is_err());
    assert_eq!(snapshot(&parked.join("destination")), original);
    assert_eq!(fs::read(&target).unwrap(), b"replacement destination");
    assert_eq!(stages(&parent).len(), 1);
    assert_eq!(
        fs::read(&stages(&parent)[0]).unwrap(),
        b"other operation stage"
    );
    assert!(
        stages(&parked).is_empty(),
        "owned stage must be cleaned in the original directory"
    );
}

/// Relocating the same staging inode makes pathname custody checks pass in 1A.
fn relocate_hook(parent: PathBuf, parked: PathBuf) -> super::Reset {
    install_hook(move |event, stage| {
        if event == Event::BeforePublish {
            fs::rename(&parent, &parked)?;
            fs::create_dir(&parent)?;
            fs::write(parent.join("destination"), "replacement destination")?;
            fs::write(
                parent.join(".archivefs-config-write-other.tmp"),
                "other operation stage",
            )?;
            fs::rename(parked.join(stage.file_name().unwrap()), stage)?;
        }
        Ok(())
    })
}

#[test]
fn renamed_parent_and_relocated_stage_cannot_redirect_publication() {
    let root = fixture();
    let parent = root.path().join("parent");
    let parked = root.path().join("original-directory");
    fs::create_dir(&parent).unwrap();
    let target = parent.join("destination");
    fs::write(&target, "original destination").unwrap();
    let original = snapshot(&target);
    let before = tree(&parent);
    let _reset = relocate_hook(parent.clone(), parked.clone());
    let result = write(&target, "new text");
    evidence(
        "relocated-owned-stage",
        before,
        json!({"original":tree(&parked),"replacement":tree(&parent)}),
        &result,
    );
    assert!(
        result.is_err(),
        "must refuse rebinding rather than report redirected publication successful"
    );
    assert_eq!(snapshot(&parked.join("destination")), original);
    assert_eq!(fs::read(&target).unwrap(), b"replacement destination");
    assert_eq!(
        fs::read(parent.join(".archivefs-config-write-other.tmp")).unwrap(),
        b"other operation stage"
    );
    assert_eq!(
        stages(&parent).len(),
        2,
        "relocated stage outside the pinned directory must be retained"
    );
}

#[test]
fn symlink_rebinding_and_relocated_stage_cannot_redirect_publication() {
    let root = fixture();
    let original = root.path().join("original-directory");
    let replacement = root.path().join("replacement-directory");
    let parent = root.path().join("parent-link");
    fs::create_dir(&original).unwrap();
    fs::create_dir(&replacement).unwrap();
    symlink(&original, &parent).unwrap();
    fs::write(original.join("destination"), "original destination").unwrap();
    fs::write(replacement.join("destination"), "replacement destination").unwrap();
    let before = json!({"parent":object(&parent),"original":tree(&original),"replacement":tree(&replacement)});
    let (p, a, b) = (parent.clone(), original.clone(), replacement.clone());
    let _reset = install_hook(move |event, stage| {
        if event == Event::BeforePublish {
            fs::rename(
                a.join(stage.file_name().unwrap()),
                b.join(stage.file_name().unwrap()),
            )?;
            fs::remove_file(&p)?;
            symlink(&b, &p)?;
        }
        Ok(())
    });
    let result = write(&parent.join("destination"), "new text");
    evidence(
        "symlink-rebinding",
        before,
        json!({"parent":object(&parent),"original":tree(&original),"replacement":tree(&replacement)}),
        &result,
    );
    assert!(result.is_err());
    assert_eq!(
        fs::read(original.join("destination")).unwrap(),
        b"original destination"
    );
    assert_eq!(
        fs::read(replacement.join("destination")).unwrap(),
        b"replacement destination"
    );
    assert_eq!(stages(&replacement).len(), 1);
}

#[test]
fn journal_writer_propagates_boundary_refusal() {
    use crate::dat::rename_apply::{RenameTransaction, TransactionState, write_journal};
    let root = fixture();
    let parent = root.path().join("parent");
    let parked = root.path().join("original-directory");
    let mut transaction = RenameTransaction {
        transaction_id: "destination".into(),
        plan_generation: 7,
        classifier_version: Some(crate::dat::classification::CLASSIFIER_VERSION.into()),
        created_at_unix: 42,
        source_scan_root: "synthetic".into(),
        state: TransactionState::Planned,
        entries: vec![],
        created_directories: vec![],
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    write_journal(&parent, &transaction).unwrap();
    let before = tree(&parent);
    let original = snapshot(&parent.join("destination.json"));
    let (p, a) = (parent.clone(), parked.clone());
    let _reset = install_hook(move |event, stage| {
        if event == Event::BeforePublish {
            fs::rename(&p, &a)?;
            fs::create_dir(&p)?;
            fs::write(p.join("destination.json"), "unrelated journal bytes")?;
            fs::rename(a.join(stage.file_name().unwrap()), stage)?;
        }
        Ok(())
    });
    transaction.state = TransactionState::Applied;
    let result = write_journal(&parent, &transaction);
    evidence(
        "journal-result",
        before,
        json!({"original":tree(&parked),"replacement":tree(&parent)}),
        &result,
    );
    assert!(
        result.is_err(),
        "a redirected Applied receipt must not return Ok"
    );
    assert_eq!(snapshot(&parked.join("destination.json")), original);
    assert_eq!(
        fs::read(parent.join("destination.json")).unwrap(),
        b"unrelated journal bytes"
    );
}

#[test]
fn es_de_outer_result_propagates_recovery_publication_refusal() {
    use crate::launch::es_de_publish::{
        EsDeGamelistPublication, EsDePublicationEntry, apply_es_de_gamelist_publication,
    };
    let root = fixture();
    let parent = root.path().join("parent");
    let parked = root.path().join("original-directory");
    fs::create_dir(&parent).unwrap();
    let target = parent.join("gamelist.xml");
    fs::write(&target, "original gamelist").unwrap();
    let original = snapshot(&target);
    let before = tree(&parent);
    let publication = EsDeGamelistPublication {
        es_de_system: "nes",
        gamelist_path: target.clone(),
        previous_content: Some("original gamelist".into()),
        new_content: "new gamelist".into(),
        added: vec![EsDePublicationEntry {
            dat_entry_name: "synthetic".into(),
            destination_path: root.path().join("synthetic.txt"),
        }],
        already_present: vec![],
    };
    let (p, a) = (parent.clone(), parked.clone());
    let mut replaced = false;
    let _reset = install_hook(move |event, stage| {
        if event == Event::BeforePublish && !replaced {
            replaced = true;
            fs::rename(&p, &a)?;
            fs::create_dir(&p)?;
            fs::write(p.join("gamelist.xml"), "replacement gamelist")?;
            fs::rename(a.join(stage.file_name().unwrap()), stage)?;
        }
        Ok(())
    });
    let result = apply_es_de_gamelist_publication(&publication);
    evidence(
        "es-de-outer-result",
        before,
        json!({"original":tree(&parked),"replacement":tree(&parent)}),
        &result,
    );
    assert!(
        result.is_err(),
        "outer success must not hide redirected recovery/gamelist publication"
    );
    assert_eq!(snapshot(&parked.join("gamelist.xml")), original);
    assert_eq!(fs::read(&target).unwrap(), b"replacement gamelist");
}
