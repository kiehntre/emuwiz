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
pub(super) fn workspace(root: &Path) -> Result<PathBuf, String> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .ok_or("No private application-data directory")?;
    if !base.is_absolute() {
        return Err("Application-data directory must be absolute".into());
    }
    Ok(base
        .join("emuwiz-r2-blue")
        .join(hex(Sha256::digest(root.as_os_str().as_encoded_bytes()))))
}
#[derive(Clone)]
pub(super) struct Info {
    pub root: PathBuf,
    pub workspace: PathBuf,
    pub state: PathBuf,
    pub next_state: PathBuf,
    pub signer: PathBuf,
    pub status: Value,
}
impl Info {
    pub fn load(h: &Helper, root: &Path, workspace: &Path, init: bool) -> Result<Self, String> {
        let v = h.run(vec![
            if init { "blue-init" } else { "blue-info" }.into(),
            text(root)?,
            text(workspace)?,
        ])?;
        Self::from(v, workspace)
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
    h.run(vec![
        "enroll".into(),
        text(&info.state)?,
        text(&p.file)?,
        "--confirm".into(),
        p.confirmation.clone(),
        "--enable-contract-1-i".into(),
    ])?;
    let r = review(h, info, "apply", Some(p.selection.clone()))?;
    check_plan(&expected, &r.value["transaction"])?;
    Ok(r)
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
    let expected = selection.validate()?;
    let r = review(h, i, "apply", Some(selection))?;
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
    if let Some(s) = &r.selection {
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
    let result = h.call(
        &[
            "execute".into(),
            text(&info.state)?,
            r.action.clone(),
            text(&file)?,
            "--enable-contract-1-i".into(),
        ],
        true,
    )?;
    Ok((result, info.refresh(h)?))
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
