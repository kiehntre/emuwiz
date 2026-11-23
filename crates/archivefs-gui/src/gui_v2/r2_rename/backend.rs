//! All blocking work, including the short-lived operator/signing subprocess.
use archivefs_core::dat::{
    rename_apply::model::RenameTransaction,
    rom_organisation::{OrganisationMode, OrganisationPlan, OrganisationStatus},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
};
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Selection {
    pub plan: OrganisationPlan,
    pub approved: BTreeSet<String>,
}
impl Selection {
    pub fn fingerprint(&self) -> Result<String, String> {
        serde_json::to_vec(self).map(|bytes|hex(Sha256::digest(bytes))).map_err(|_|"A selected path is not UTF-8; Advanced Protection cannot represent this preview. You can explicitly choose Standard Rename instead.".into())
    }
    pub fn pairs(&self) -> Result<(PathBuf, Vec<(String, String)>), String> {
        self.fingerprint()?;
        if !matches!(
            self.plan.mode,
            OrganisationMode::RenameInPlace | OrganisationMode::MoveRealFile
        ) {
            return Err(
                "Advanced Protection supports regular-file Rename/Move only; links are ineligible."
                    .into(),
            );
        }
        let entries = self
            .plan
            .entries
            .iter()
            .filter(|e| {
                e.status == OrganisationStatus::Suggested
                    && self
                        .approved
                        .contains(&e.source_path.to_string_lossy().into_owned())
            })
            .collect::<Vec<_>>();
        if self.plan.entries.iter().any(|e| {
            self.approved
                .contains(&e.source_path.to_string_lossy().into_owned())
                && e.status != OrganisationStatus::Suggested
        }) || entries.len() != self.approved.len()
        {
            return Err("A selected file is no longer eligible. Regenerate the preview; no selected file will be silently omitted.".into());
        }
        if entries.is_empty() || entries.len() > 8 {
            return Err(
                "Choose one to eight files in the existing preview. Nothing is silently omitted."
                    .into(),
            );
        }
        let root = entries[0]
            .source_path
            .parent()
            .ok_or("Source has no parent")?
            .to_path_buf();
        let mut pairs = Vec::new();
        for e in entries {
            if e.source_path.parent() != Some(root.as_path())
                || e.destination_path.parent() != Some(root.as_path())
            {
                return Err("Advanced Protection currently supports files directly inside one collection folder. Cross-folder moves and nested destinations are refused.".into());
            }
            pairs.push((
                e.source_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or("Filename is not UTF-8")?
                    .into(),
                e.destination_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or("Filename is not UTF-8")?
                    .into(),
            ));
        }
        Ok((root, pairs))
    }
    pub fn validate(&self) -> Result<RenameTransaction, String> {
        crate::rom_organisation_page::validate_r2_selection(&self.plan, &self.approved)
    }
}
#[derive(Clone)]
pub(super) struct Helper {
    path: PathBuf,
}
impl Helper {
    pub fn bundled() -> Result<Self, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        Ok(Self {
            path: exe
                .parent()
                .ok_or("Missing application directory")?
                .join("emuwiz-cli"),
        })
    }
    #[cfg(test)]
    pub fn fixture(path: PathBuf) -> Self {
        Self { path }
    }
    pub fn call(&self, args: &[String], allow_status: bool) -> Result<Value, String> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let m=std::fs::symlink_metadata(&self.path).map_err(|_|"The bundled operator is unavailable. Install the matching EmuWiz CLI alongside Blue; normal Rename/Move will not be used instead.".to_string())?;
            if !m.is_file()
                || m.mode() & 0o022 != 0
                || std::fs::canonicalize(&self.path).map_err(|e| e.to_string())? != self.path
            {
                return Err("Bundled operator path is not a trusted regular executable".into());
            }
        }
        let out = Command::new(&self.path)
            .arg("--r2-operator")
            .args(args)
            .output()
            .map_err(|e| format!("Operator unavailable: {e}"))?;
        if !out.status.success() && !allow_status {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
        }
        serde_json::from_slice(&out.stdout).map_err(|_|format!("Operator refused or returned incomplete evidence: {}. Preserve the safety workspace and refresh current state.",String::from_utf8_lossy(&out.stderr).trim()))
    }
    pub fn run(&self, args: Vec<String>) -> Result<Value, String> {
        self.call(&args, false)
    }
}
fn text(p: &Path) -> Result<String, String> {
    p.to_str()
        .map(str::to_owned)
        .ok_or("Path is not UTF-8".into())
}
/// Advanced Protection's private recovery workspace for `root` (owner option
/// 1, 2026-11-23): `<folder holding your library folder>/.emuwiz-protection/
/// <root key>`, on the same disk as the games (the engine requires that), outside
/// every library folder (so no scan reads it) and created owner-only (0700).
pub(super) fn workspace(root: &Path) -> Result<PathBuf, String> {
    let database = archivefs_core::default_database_path().map_err(|e| e.to_string())?;
    let library = archivefs_core::Database::open_read_only(&database).map_err(|e| e.to_string())?;
    let libraries = library
        .list_source_folders()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|folder| folder.path)
        .collect::<Vec<_>>();
    workspace_for(root, &libraries)
}

pub(super) fn workspace_for(root: &Path, libraries: &[PathBuf]) -> Result<PathBuf, String> {
    use std::os::unix::fs::MetadataExt;
    let library = libraries
        .iter()
        .filter(|library| root.starts_with(library))
        .max_by_key(|library| library.components().count())
        .ok_or("Advanced Protection works only on a folder inside one of your library folders. Nothing was changed.")?;
    let beside = library.parent().ok_or(
        "Your library folder has no parent folder for the recovery workspace. Nothing was changed.",
    )?;
    let shown = beside.display();
    let meta = std::fs::symlink_metadata(beside).map_err(|e| format!("{shown}: {e}"))?;
    if !meta.is_dir() || std::fs::canonicalize(beside).ok().as_deref() != Some(beside) {
        return Err(format!(
            "Advanced Protection keeps its private recovery folder in {shown}, which must be a real folder (not a link). Nothing was changed."
        ));
    }
    // SAFETY: plain syscall.
    if meta.uid() != unsafe { libc::geteuid() } {
        return Err(format!(
            "Advanced Protection keeps its private recovery folder in {shown}, which must be owned by you. Nothing was changed."
        ));
    }
    if meta.mode() & 0o022 != 0 {
        return Err(format!(
            "Advanced Protection keeps its private recovery folder in {shown}, which other users can change (permissions {:o}). To use it, remove their write access yourself, for example: chmod go-w '{shown}'. Nothing was changed.",
            meta.mode() & 0o777
        ));
    }
    if std::fs::metadata(root).map_err(|e| e.to_string())?.dev() != meta.dev() {
        return Err(format!(
            "Advanced Protection needs its recovery folder on the same disk as the games, but {shown} is on another disk (your library folder is a disk's top folder). Nothing was changed."
        ));
    }
    if libraries.iter().any(|other| beside.starts_with(other)) {
        return Err(format!(
            "{shown} is itself inside a library folder, so a recovery folder there would be scanned as games. Advanced Protection cannot be used for this library. Nothing was changed."
        ));
    }
    Ok(beside
        .join(pc::WORKSPACE_FOLDER)
        .join(hex(Sha256::digest(root.as_os_str().as_encoded_bytes()))))
}

use archivefs_core::dat::rom_organisation::protected_catalogue as pc;

#[derive(Clone)]
pub(super) struct Info {
    pub root: PathBuf,
    pub workspace: PathBuf,
    pub state: PathBuf,
    pub next_state: PathBuf,
    pub signer: PathBuf,
    pub status: Value,
    /// Notes from finishing interrupted library steps on load.
    pub catalogue_notes: Vec<String>,
    /// Operations whose library step cannot be finished from evidence; a
    /// library scan refuses until each is reviewed here.
    pub unresolved: Vec<pc::Unresolved>,
}
impl Info {
    pub fn load(h: &Helper, root: &Path, workspace: &Path, init: bool) -> Result<Self, String> {
        let v = h.run(vec![
            if init { "blue-init" } else { "blue-info" }.into(),
            text(root)?,
            text(workspace)?,
        ])?;
        let mut info = Self::from(v, workspace)?;
        // Never fails the page: it must open to review what blocks a scan.
        match pc::WorkspaceLock::try_take(workspace) {
            Ok(Some(_lock)) => info.unresolved = pc::unresolved_in(workspace),
            Ok(None) => info
                .catalogue_notes
                .push("Another Advanced Protection step is running for this folder.".into()),
            Err(e) => info.catalogue_notes.push(e),
        }
        Ok(info)
    }
    fn from(v: Value, workspace: &Path) -> Result<Self, String> {
        let path = |k: &str| {
            v[k].as_str()
                .map(PathBuf::from)
                .ok_or_else(|| format!("Missing operator {k}"))
        };
        Ok(Self {
            root: path("root")?,
            workspace: workspace.into(),
            state: path("state")?,
            next_state: path("next_state")?,
            signer: path("signer")?,
            status: v["status"].clone(),
            catalogue_notes: Vec::new(),
            unresolved: Vec::new(),
        })
    }
    pub fn refresh(&self, h: &Helper) -> Result<Self, String> {
        Self::load(h, &self.root, &self.workspace, false)
    }
}
#[derive(Clone)]
pub(super) struct Preview {
    pub selection: Selection,
    pub plan: Value,
    pub confirmation: String,
    pub file: PathBuf,
}
#[derive(Clone)]
pub(super) struct Review {
    pub action: String,
    pub value: Value,
    pub selection: Option<Selection>,
}
pub(super) fn prepare(h: &Helper, info: &Info, selection: Selection) -> Result<Preview, String> {
    let (root, pairs) = selection.pairs()?;
    if root != info.root {
        return Err(
            "Selected preview belongs to another collection. Open that collection explicitly."
                .into(),
        );
    }
    let expected = selection.validate()?;
    let file = info.workspace.join(format!("plan-{}.json", nonce()));
    let mut args = vec!["plan".into(), text(&info.state)?, text(&file)?];
    for (a, b) in pairs {
        args.push(a);
        args.push(b);
    }
    let v = h.run(args)?;
    check_plan(&expected, &v["plan"])?;
    Ok(Preview {
        selection,
        plan: v["plan"].clone(),
        confirmation: v["confirmation"]
            .as_str()
            .ok_or("Missing plan digest")?
            .into(),
        file,
    })
}
fn nonce() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        COUNT.fetch_add(1, Ordering::Relaxed)
    )
}
fn check_plan(expected: &RenameTransaction, observed: &Value) -> Result<(), String> {
    let tx: RenameTransaction =
        serde_json::from_value(observed.clone()).map_err(|e| e.to_string())?;
    if tx.entries.len() != expected.entries.len()
        || tx.entries.iter().zip(&expected.entries).any(|(a, b)| {
            a.source_path != b.source_path
                || a.destination_path != b.destination_path
                || a.identity != b.identity
        })
    {
        return Err(
            "Files, identity or selected plan changed. Generate and review a new preview.".into(),
        );
    }
    Ok(())
}
pub(super) fn enroll(h: &Helper, info: &Info, p: &Preview) -> Result<Review, String> {
    let expected = p.selection.validate()?;
    check_plan(&expected, &p.plan)?;
    let inspected = h.run(vec!["inspect-plan".into(), text(&p.file)?])?;
    if inspected["plan"] != p.plan || inspected["confirmation"] != p.confirmation {
        return Err("Reviewed plan document changed; enrollment refused".into());
    }
    // The enrollment record (exact file state and the saved checks current
    // now) is written first, under the workspace lock, so Apply can confirm
    // these files again after the recovery anchors are linked to them.
    let _lock = pc::WorkspaceLock::try_take(&info.workspace)?.ok_or(
        "Another Advanced Protection step is running for this folder. Nothing was changed.",
    )?;
    if pc::involves_catalogue(&expected) {
        let id = p.plan["transaction_id"]
            .as_str()
            .ok_or("The reviewed plan has no transaction id; enrollment refused")?;
        pc::record_enrollment(&info.workspace, &expected, id)?;
    }
    h.run(vec![
        "enroll".into(),
        text(&info.state)?,
        text(&p.file)?,
        "--confirm".into(),
        p.confirmation.clone(),
        "--enable-contract-1-i".into(),
    ])?;
    drop(_lock);
    let r = review(h, info, "apply", Some(p.selection.clone()))?;
    check_plan(&expected, &r.value["transaction"])?;
    Ok(r)
}

/// The saved checks of the enrolled Apply's own files, confirmed again
/// (owner approval 2026-11-23): its recovery anchors changed their change
/// time and link count. Only this transaction's files; anything else stays
/// stale and the usual refusal follows.
fn confirm_enrolled(info: &Info, transaction: &Value) -> Result<(), String> {
    let operator: RenameTransaction =
        serde_json::from_value(transaction.clone()).map_err(|e| e.to_string())?;
    pc::confirm_enrolled(&info.workspace, &operator, &info.root).map(|_| ())
}
pub(super) fn review(
    h: &Helper,
    info: &Info,
    action: &str,
    selection: Option<Selection>,
) -> Result<Review, String> {
    let value = h.run(vec!["review".into(), text(&info.state)?, action.into()])?;
    if value["action"]
        != match action {
            "apply" => "Apply",
            "undo" => "Undo",
            _ => "RetainForeign",
        }
    {
        return Err("Operator review action differs from request".into());
    }
    Ok(Review {
        action: action.into(),
        value,
        selection,
    })
}
pub(super) fn review_apply(h: &Helper, i: &Info, selection: Selection) -> Result<Review, String> {
    let r = review(h, i, "apply", Some(selection.clone()))?;
    {
        let _lock = pc::WorkspaceLock::try_take(&i.workspace)?.ok_or(
            "Another Advanced Protection step is running for this folder. Nothing was changed.",
        )?;
        confirm_enrolled(i, &r.value["transaction"])?;
    }
    let expected = selection.validate()?;
    check_plan(&expected, &r.value["transaction"])?;
    Ok(r)
}
pub(super) fn execute(h: &Helper, info: &Info, r: &Review) -> Result<(Value, Info), String> {
    if r.action == "apply" && r.selection.is_none() {
        return Err("Apply requires the exact current organisation selection".into());
    }
    if r.value["actionable"] != true {
        return Err(r.value["refusal"]
            .as_str()
            .unwrap_or("Operation requires review")
            .into());
    }
    // The library catalogue follows the files (owner option 1). Under the
    // workspace lock (a scan waits for it) and the catalogue hold: the
    // record is written as pending with the checks current now, the engine
    // moves the files, then the library is brought in line file by file. An
    // interruption is finished by the next scan or visit, from this record.
    let _lock = pc::WorkspaceLock::try_take(&info.workspace)?.ok_or(
        "Another Advanced Protection step is running for this folder. Nothing was changed.",
    )?;
    if let Some(s) = &r.selection {
        confirm_enrolled(info, &r.value["transaction"])?;
        let tx = s.validate()?;
        check_plan(&tx, &r.value["transaction"])?;
    }
    let seal = r.value["confirmation"]
        .as_str()
        .ok_or("Missing review digest")?;
    let file = info.workspace.join(format!("grant-{}.json", nonce()));
    h.run(vec![
        "approve".into(),
        text(&info.state)?,
        text(&info.signer)?,
        r.action.clone(),
        "--confirm".into(),
        seal.into(),
        text(&file)?,
    ])?;
    let inspected = h.call(
        &[
            "inspect-grant".into(),
            text(&info.state)?,
            r.action.clone(),
            text(&file)?,
        ],
        true,
    )?;
    if inspected["verdict"] != "usable" {
        return Err(
            "Fresh approval is not applicable. Preserve it and review current state.".into(),
        );
    }
    let mut catalogue = catalogue_step(info, r)?;
    let hold = match &mut catalogue {
        Some((path, record)) => {
            let reverse = record.pending().unwrap_or_default();
            let hold = pc::CatalogueHold::take(
                &archivefs_core::default_database_path().map_err(|e| e.to_string())?,
            )?;
            record.checked = pc::checks_before_protected(&record.transaction, reverse);
            pc::write_record(path, record)?;
            Some(hold)
        }
        None => None,
    };
    let result = h.call(
        &[
            "execute".into(),
            text(&info.state)?,
            r.action.clone(),
            text(&file)?,
            "--enable-contract-1-i".into(),
        ],
        true,
    );
    drop(hold);
    fault_after_protected_move();
    let mut notes = Vec::new();
    if let Some((path, mut record)) = catalogue {
        notes = pc::settle_record(&path, &mut record)?;
    }
    let mut result = result?;
    result["catalogue"] = serde_json::json!(notes);
    drop(_lock);
    Ok((result, info.refresh(h)?))
}

/// Acceptance fault point (debug builds only, owner 2026-11-23): when the
/// one-shot marker `fault-after-protected-move` is in the profile's data
/// folder, Blue stops right after the engine moved the files and before the
/// library step, exactly as an interruption there would. The marker is
/// renamed to `.used` so it fires once. Release builds have no such point.
fn fault_after_protected_move() {
    #[cfg(debug_assertions)]
    {
        let Some(data) = std::env::var_os("EMUWIZ_DATA_HOME") else {
            return;
        };
        let marker = PathBuf::from(data).join("fault-after-protected-move");
        if std::fs::rename(&marker, marker.with_extension("used")).is_ok() {
            std::process::exit(86);
        }
    }
}

/// The record this execution's library step uses (not yet written): a new
/// one for Apply (the preview's own transaction with its sealed library
/// bindings, checked again now), or the applied one an Undo reverses.
/// `None` when no library entry is involved.
fn catalogue_step(info: &Info, r: &Review) -> Result<Option<(PathBuf, pc::PendingRecord)>, String> {
    match (r.action.as_str(), &r.selection) {
        ("apply", Some(selection)) => {
            let transaction = selection.validate()?;
            if !pc::involves_catalogue(&transaction) {
                return Ok(None);
            }
            Ok(Some((
                info.workspace.join(format!("catalogue-{}.json", nonce())),
                pc::PendingRecord {
                    version: 1,
                    created_ns: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos(),
                    state: "apply_pending".into(),
                    transaction,
                    checked: Default::default(),
                },
            )))
        }
        ("undo", _) => {
            let Some((path, mut record)) =
                pc::records(&info.workspace)?
                    .into_iter()
                    .rev()
                    .find(|(_, record)| {
                        record.state == "applied"
                            && check_plan(&record.transaction, &r.value["transaction"]).is_ok()
                    })
            else {
                return Ok(None);
            };
            record.state = "undo_pending".into();
            Ok(Some((path, record)))
        }
        _ => Ok(None),
    }
}
/// The reviewed way out for operations whose library step cannot be
/// proved: their records are kept (renamed `reviewed-…`), and the next scan
/// treats the files as moved outside EmuWiz.
pub(super) fn release_for_rescan(h: &Helper, info: &Info) -> Result<Info, String> {
    let _lock = pc::WorkspaceLock::try_take(&info.workspace)?.ok_or(
        "Another Advanced Protection step is running for this folder. Nothing was changed.",
    )?;
    for unresolved in &info.unresolved {
        pc::release_for_rescan(&unresolved.record)?;
    }
    drop(_lock);
    info.refresh(h)
}

pub(super) fn reuse_preview(h: &Helper, i: &Info) -> Result<Value, String> {
    h.run(vec![
        "preview-reuse".into(),
        text(&i.state)?,
        text(&i.next_state)?,
    ])
}
pub(super) fn reuse(h: &Helper, i: &Info, r: &Value) -> Result<Info, String> {
    if r["next_state"].as_str() != i.next_state.to_str() {
        return Err("Next transaction selection changed".into());
    }
    h.run(vec![
        "reuse".into(),
        text(&i.state)?,
        text(&i.next_state)?,
        "--confirm".into(),
        r["confirmation"]
            .as_str()
            .ok_or("Missing reuse review")?
            .into(),
        "--enable-contract-1-i".into(),
    ])?;
    i.refresh(h)
}
pub(super) fn history(h: &Helper, i: &Info) -> Result<Value, String> {
    h.run(vec!["history".into(), text(&i.state)?])
}
pub(super) fn retrieve(
    h: &Helper,
    i: &Info,
    index: usize,
    retained: &Value,
    dest: &Path,
) -> Result<Value, String> {
    let seal = hex(Sha256::digest(
        serde_json::to_vec(retained).map_err(|e| e.to_string())?,
    ));
    h.run(vec![
        "blue-retrieve".into(),
        text(&i.state)?,
        index.to_string(),
        "--confirm".into(),
        seal,
        text(dest)?,
    ])
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

pub(super) fn retrieve_historical(
    h: &Helper,
    i: &Info,
    receipt: &Value,
    dest: &Path,
) -> Result<Value, String> {
    let status = h.call(&["status".into(), text(&i.state)?], true)?;
    if !super::view::verified_inventory(&status) {
        return Err(
            "Earlier retained inventory requires operator review; no copy was created.".into(),
        );
    }
    let entry = status["entries"]
        .as_array()
        .and_then(|rows| rows.iter().find(|e| e["retained_foreign"] == *receipt))
        .ok_or("Retained evidence changed; load History again")?;
    let index = entry["index"]
        .as_u64()
        .ok_or("Missing retained entry index")? as usize;
    retrieve(h, i, index, receipt, dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changed_review_path_or_identity_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let source = t.path().join("a.bin");
        std::fs::write(&source, b"synthetic").unwrap();
        let value = serde_json::json!({"transaction_id":"synthetic-review","plan_generation":1,"created_at_unix":1,"source_scan_root":t.path(),"entries":[{"source_path":source,"destination_path":t.path().join("b.bin"),"original_basename":"a.bin","proposed_basename":"b.bin","identity":archivefs_core::dat::rename_apply::identity::capture_identity(&source).unwrap()}]});
        let expected: RenameTransaction = serde_json::from_value(value.clone()).unwrap();
        assert!(check_plan(&expected, &value).is_ok());
        let mut changed = value.clone();
        changed["entries"][0]["destination_path"] = serde_json::json!(t.path().join("other.bin"));
        assert!(check_plan(&expected, &changed).is_err());
        changed = value.clone();
        changed["entries"][0]["identity"]["ino"] = serde_json::json!(0);
        assert!(check_plan(&expected, &changed).is_err());
        changed = value;
        changed["entries"] = serde_json::json!([]);
        assert!(check_plan(&expected, &changed).is_err());
    }
}
