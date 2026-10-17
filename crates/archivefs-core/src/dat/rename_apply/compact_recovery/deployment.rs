//! Limited Contract 1-I provisioning and operator workflow. No ambient activation.
//! Registration pins the original plan/ticket for the existing signed protocol;
//! it is immutable trust configuration, not another transaction journal.
use super::*;
use crate::dat::rename_apply::identity::{capture_identity, fd_publication_probe::contract};
use ring::signature::{Ed25519KeyPair, KeyPair};
use std::io::{Read, Write};
use std::os::unix::fs::DirBuilderExt;

const FORMAT: &str = "emuwiz-r2-deployment-v1";
const ACCEPT: &str = "Contract1-I";
const BINDING: &str = "deployment.json";
const MAX_RECORD: u64 = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    format: String,
    root: PathBuf,
    root_id: (u64, u64),
    state: PathBuf,
    state_id: (u64, u64),
    operator: String,
    authority: admission::Configuration,
    verifier: approval::Verifier,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Registration {
    original: Vec<u8>,
    ticket: Ticket,
}
#[derive(Debug, Serialize)]
pub struct Execution {
    pub completed: bool,
    pub transaction: RenameTransaction,
}
#[derive(Serialize)]
pub struct Review {
    pub action: RequestedAction,
    pub confirmation: String,
    pub operator: String,
    pub transaction: RenameTransaction,
    pub checkpoint_pending: bool,
    pub holding: serde_json::Value,
    pub actionable: bool,
    pub eligible_entries: Vec<usize>,
    pub refusal: Option<String>,
    pub operator_effect: String,
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn hash(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
fn accept(value: &str) -> io::Result<()> {
    if value != ACCEPT {
        return Err(fail(
            "explicit Contract1-I acceptance required; default is OFF",
        ));
    }
    Ok(())
}
fn enabled(value: bool) -> io::Result<controller::RuntimeSwitch> {
    if !value {
        return Err(fail("R2 is OFF; explicit per-command activation required"));
    }
    Ok(controller::RuntimeSwitch::enabled_for_evaluation())
}
fn directory(path: &Path) -> io::Result<File> {
    if std::fs::canonicalize(path)? != path {
        return Err(fail("canonical private directory required"));
    }
    let f = open_dir(path)?;
    private(&f)?;
    Ok(f)
}
fn create_directory(path: &Path) -> io::Result<File> {
    let parent = path.parent().ok_or_else(|| fail("missing parent"))?;
    let pinned = directory(parent)?;
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(fail("absolute new directory required"));
    }
    std::fs::DirBuilder::new().mode(0o700).create(path)?;
    let result = directory(path)?;
    if identity(&directory(parent)?)? != identity(&pinned)? {
        return Err(fail(
            "provisioning parent changed; retain partial setup for review",
        ));
    }
    result.sync_all()?;
    pinned.sync_all()?;
    Ok(result)
}
fn write_new(dir: &File, name: &str, bytes: &[u8]) -> io::Result<()> {
    if bytes.len() as u64 > MAX_RECORD {
        return Err(fail("deployment record size limit"));
    }
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(at(dir, name))?;
    f.write_all(bytes)?;
    f.sync_all()?;
    dir.sync_all()
}
fn read(dir: &File, name: &str) -> io::Result<Vec<u8>> {
    let mut f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(at(dir, name))?;
    private(&f)?;
    let m = f.metadata()?;
    if !m.is_file() || m.nlink() != 1 || m.len() > MAX_RECORD {
        return Err(fail("invalid private deployment record"));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut f)
        .take(MAX_RECORD + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RECORD {
        return Err(fail("deployment record grew"));
    }
    Ok(bytes)
}
fn capacity(dir: &File) -> io::Result<()> {
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::fstatvfs(dir.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let stat = unsafe { stat.assume_init() };
    if u128::from(stat.f_bavail) * u128::from(stat.f_frsize) < u128::from(CAP + MAX_RECORD) {
        return Err(fail(
            "insufficient free space for bounded recovery setup; no enrollment",
        ));
    }
    Ok(())
}
/// Creates a NEW private signer directory. No existing key/permissions overwritten.
/// Only the operator approval command ever loads this signing key.
pub fn issuer_init(path: &Path) -> io::Result<Vec<u8>> {
    let dir = create_directory(path)?;
    let key = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
        .map_err(|_| fail("key generation failed"))?;
    let pair =
        Ed25519KeyPair::from_pkcs8(key.as_ref()).map_err(|_| fail("invalid generated key"))?;
    write_new(&dir, "signing.pk8", key.as_ref())?;
    let public = pair.public_key().as_ref().to_vec();
    write_new(
        &dir,
        "public.json",
        &serde_json::to_vec(&public).map_err(fail)?,
    )?;
    Ok(public)
}
/// Provision one authority for one root. Remains OFF; no game file enrollment.
pub fn provision(
    root: &Path,
    state: &Path,
    public_key: &[u8],
    operator: &str,
    contract: &str,
) -> io::Result<()> {
    accept(contract)?;
    if public_key.len() != 32 || operator.trim().is_empty() || operator.len() > 128 {
        return Err(fail("valid Ed25519 public key and named operator required"));
    }
    let root_dir = directory(root)?;
    capacity(&root_dir)?;
    if state.starts_with(root) || root.starts_with(state) {
        return Err(fail(
            "authority and collection must be separate private directories",
        ));
    }
    for reserved in [NAMESPACE, ".emuwiz-r2-holding"] {
        if std::fs::symlink_metadata(root.join(reserved)).is_ok() {
            return Err(fail(
                "root already has recovery/provisioning evidence; review required",
            ));
        }
    }
    if directory(state.parent().ok_or_else(|| fail("missing state parent"))?)?
        .metadata()?
        .dev()
        != root_dir.metadata()?.dev()
    {
        return Err(fail("authority must share the eligible local filesystem"));
    }
    contract::capability::probe(root)?;
    let dir = create_directory(state)?;
    capacity(&dir)?;
    create_directory(&state.join("admission"))?;
    let authority = admission::Configuration::create(&state.join("admission"))?;
    let grants = create_directory(&state.join("grants"))?;
    let config = Configuration {
        format: FORMAT.into(),
        root: root.into(),
        root_id: identity(&root_dir)?,
        state: state.into(),
        state_id: identity(&dir)?,
        operator: operator.into(),
        authority,
        verifier: approval::Verifier {
            directory: state.join("grants"),
            identity: identity(&grants)?,
            public_key: public_key.into(),
        },
    };
    let bytes = serde_json::to_vec(&config).map_err(fail)?;
    // Unique root registration prevents another participating deployment from
    // silently selecting another authority. Partial setup is never overwritten.
    write_new(
        &directory(&root.join(".emuwiz-r2-holding"))?,
        BINDING,
        hash(&bytes).as_bytes(),
    )?;
    write_new(&dir, "configuration.json", &bytes)?;
    load(state)?;
    Ok(())
}
fn load(state: &Path) -> io::Result<Configuration> {
    let dir = directory(state)?;
    let bytes = read(&dir, "configuration.json")?;
    let c: Configuration = serde_json::from_slice(&bytes).map_err(fail)?;
    if c.format != FORMAT
        || c.state != state
        || c.state_id != identity(&dir)?
        || c.root_id != identity(&directory(&c.root)?)?
    {
        return Err(fail("deployment configuration/root identity mismatch"));
    }
    if read(&directory(&c.root.join(".emuwiz-r2-holding"))?, BINDING)? != hash(&bytes).as_bytes() {
        return Err(fail("collection belongs to different deployment authority"));
    }
    c.verifier.open()?;
    Ok(c)
}
/// Plan explicit direct-child filename pairs. No enrollment or mutation authority.
pub fn plan(state: &Path, pairs: &[(String, String)]) -> io::Result<RenameTransaction> {
    let c = load(state)?;
    if pairs.is_empty() || pairs.len() > 8 {
        return Err(fail("one to eight filename pairs required"));
    }
    let mut random = [0; 16];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut random).map_err(fail)?;
    let mut entries = Vec::new();
    for (source, destination) in pairs {
        for name in [source, destination] {
            let mut components = Path::new(name).components();
            if !matches!(components.next(), Some(Component::Normal(_)))
                || components.next().is_some()
            {
                return Err(fail("direct-child filenames only"));
            }
        }
        let source_path = c.root.join(source);
        entries.push(TransactionEntry {
            identity: capture_identity(&source_path)?,
            source_path,
            destination_path: c.root.join(destination),
            original_basename: source.clone(),
            proposed_basename: destination.clone(),
            operation: TransactionOperation::RenameMove,
            preflight_passed: false,
            preflight_failures: vec![],
            state: EntryState::Planned,
            failure_reason: None,
            applied_at_unix: None,
            rolled_back_at_unix: None,
            unknown: Default::default(),
        });
    }
    let tx = RenameTransaction {
        transaction_id: format!("r2-{}", hex(&random)),
        plan_generation: 1,
        classifier_version: Some(crate::dat::classification::CLASSIFIER_VERSION.into()),
        created_at_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(fail)?
            .as_secs(),
        source_scan_root: c.root.to_string_lossy().into_owned(),
        state: TransactionState::Planned,
        entries,
        created_directories: vec![],
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    let base = Authority::decode_legacy(&serde_json::to_vec(&tx).map_err(fail)?, 1024 * 1024)
        .map_err(protocol)?;
    Bridge::supported(&base)?;
    Bridge::confined(&c.root, &base)?;
    Ok(tx)
}
pub fn plan_confirmation(bytes: &[u8]) -> io::Result<String> {
    Authority::decode_legacy(bytes, 1024 * 1024).map_err(protocol)?;
    Ok(hash(bytes))
}
/// Explicit enrollment only; never Apply. Crash/partial setup retains evidence.
pub fn enroll(state: &Path, plan: &[u8], confirmation: &str, enable: bool) -> io::Result<()> {
    let switch = enabled(enable)?;
    if plan_confirmation(plan)? != confirmation {
        return Err(fail("plan confirmation differs"));
    }
    let c = load(state)?;
    let dir = directory(state)?;
    capacity(&dir)?;
    capacity(&directory(&c.root)?)?;
    if std::fs::symlink_metadata(at(&dir, "registration.json")).is_ok()
        || std::fs::symlink_metadata(c.root.join(NAMESPACE)).is_ok()
    {
        return Err(fail(
            "one enrolled transaction per root; existing evidence requires review",
        ));
    }
    let base = Authority::decode_legacy(plan, 1024 * 1024).map_err(protocol)?;
    let controller =
        controller::Controller::enroll(&c.root, &base.transaction, &c.authority, switch)?;
    let registration = Registration {
        original: controller.bridge.base.original_bytes().into(),
        ticket: controller.bridge.store.ticket.clone(),
    };
    write_new(
        &dir,
        "registration.json",
        &serde_json::to_vec(&registration).map_err(fail)?,
    )
}
fn registered(c: &Configuration) -> io::Result<(Registration, Rc<Bridge>)> {
    let r: Registration =
        serde_json::from_slice(&read(&directory(&c.state)?, "registration.json")?).map_err(fail)?;
    if r.ticket.root != c.root || r.ticket.root_id != c.root_id {
        return Err(fail("registered root differs"));
    }
    let base = Authority::decode_legacy(&r.original, 1024 * 1024).map_err(protocol)?;
    Bridge::supported(&base)?;
    Bridge::confined(&c.root, &base)?;
    if !shared_engine::selected(&base.transaction) {
        return Err(fail("shared FD registration required"));
    }
    let mut guard = c.authority.acquire()?;
    guard.resume(&base)?;
    let store = Store::reopen(&r.ticket, &base, CAP)?;
    let prefix = store.scan(&base)?.cursor;
    let b = Rc::new(Bridge {
        restarted: true,
        engine_enabled: Cell::new(false),
        issuer: RefCell::new(Some(c.verifier.clone())),
        confirmation: RefCell::new(None),
        base,
        store,
        guard: RefCell::new(guard),
        prefix: RefCell::new(prefix),
        #[cfg(test)]
        fault: RefCell::new(None),
        #[cfg(test)]
        checkpoint_count: Cell::new(0),
        #[cfg(test)]
        checkpoint_fault: RefCell::new(None),
        #[cfg(test)]
        publication_fault: RefCell::new(None),
    });
    Ok((r, b))
}
fn review_locked(c: &Configuration, b: &Bridge, action: RequestedAction) -> io::Result<Review> {
    let scan = b.store.scan(&b.base)?;
    if scan.pending.is_some()
        && let Some((_, _, pending)) = b.store.pending_transaction(&b.base)?
    {
        contract::verify_records(&pending)?;
        approval::verify_retention_receipts(&c.verifier, &b.store, &b.base, &pending)?;
    }
    let tx = scan.runtime.restore(&b.base, &scan.stream)?;
    contract::verify_records(&tx)?;
    approval::verify_retention_receipts(&c.verifier, &b.store, &b.base, &tx)?;
    let applicable = super::fd_contract_bridge::eligible_entries(
        &c.root,
        &tx,
        action,
        !scan.cursor.is_empty() || scan.pending.is_some(),
    );
    let (actionable, eligible_entries, refusal) = match applicable {
        Ok(indices) => (true, indices, None),
        Err(e) => (false, vec![], Some(e.to_string())),
    };
    let observations = serde_json::to_vec(&b.observations()?).map_err(fail)?;
    let operator_effect=match action {
        RequestedAction::RetainForeign=>"Explicit signed disposition: permanently retain the identified foreign holding files AND original continuity anchors; restore only proven originals. No deletion, automatic return or garbage collection; no ownership release. Fresh ordinary Undo is required afterwards.",
        RequestedAction::Undo=>"Recover eligible original files only; leave already parked entries untouched. Release ownership only after all entries and retained signed receipts verify.",
        RequestedAction::Apply=>"Perform the independently enrolled original Apply plan only.",
    }.to_owned();
    let seal = serde_json::to_vec(&(
        FORMAT,
        &c.operator,
        &c.verifier,
        action,
        &operator_effect,
        b.base.binding(),
        hash(&approval::container_bytes(&b.store)?),
        hash(&observations),
    ))
    .map_err(fail)?;
    Ok(Review {
        action,
        confirmation: hash(&seal),
        operator: c.operator.clone(),
        holding: contract::capability::inventory(&c.root, &tx)?,
        transaction: tx,
        checkpoint_pending: scan.pending.is_some(),
        actionable,
        eligible_entries,
        refusal,
        operator_effect,
    })
}
/// Inspect under participating admission; no approval consumption or file mutation.
pub fn review(state: &Path, action: RequestedAction) -> io::Result<Review> {
    let c = load(state)?;
    let (_, b) = registered(&c)?;
    review_locked(&c, &b, action)
}
/// Operator-only step: revalidate displayed digest, load the separate key, and
/// issue the existing durable one-shot grant. Never executes Apply/Undo.
pub fn approve(
    state: &Path,
    signer: &Path,
    action: RequestedAction,
    confirmation: &str,
) -> io::Result<approval::Approval> {
    let c = load(state)?;
    if signer.starts_with(&c.root) || signer.starts_with(state) {
        return Err(fail(
            "signer must be separate from collection and recovery configuration",
        ));
    }
    let key = read(&directory(signer)?, "signing.pk8")?;
    let (r, b) = registered(&c)?;
    let inspected = review_locked(&c, &b, action)?;
    if inspected.confirmation != confirmation {
        return Err(fail("operator review is stale or substituted"));
    }
    if !inspected.actionable {
        return Err(fail(
            inspected
                .refusal
                .unwrap_or_else(|| "action is not applicable".into()),
        ));
    }
    let issuer = approval::Issuer::new(&c.verifier.directory, r.original, r.ticket, &key)?;
    if issuer.verifier != c.verifier {
        return Err(fail("signing key differs from configured issuer"));
    }
    let context = issuer.context(&b.store, &b.base)?;
    let observations = serde_json::to_vec(&b.observations()?).map_err(fail)?;
    // Match the observation used to display/confirm, including concurrent outside writers.
    if review_locked(&c, &b, action)?.confirmation != confirmation {
        return Err(fail("operator review changed while signing"));
    }
    issuer.issue(&context, action, &observations)
}
/// Mutation process receives a grant, never a private key. OFF unless explicitly enabled.
pub fn execute(
    state: &Path,
    grant: &approval::Approval,
    action: RequestedAction,
    enable: bool,
) -> io::Result<Execution> {
    let switch = enabled(enable)?;
    let c = load(state)?;
    // Fresh capability checks do not widen eligibility or authorize mutation.
    contract::capability::probe(&c.root)?;
    let r: Registration =
        serde_json::from_slice(&read(&directory(state)?, "registration.json")?).map_err(fail)?;
    if r.ticket.root != c.root || r.ticket.root_id != c.root_id {
        return Err(fail("registered root differs"));
    }
    let controller = controller::Controller::reopen(
        &r.original,
        &c.authority,
        &c.verifier,
        grant,
        action,
        switch,
    )?;
    let cancel = AtomicBool::new(false);
    match action {
        RequestedAction::Apply => {
            let tx = controller.transaction()?;
            let approved = tx
                .entries
                .iter()
                .map(|e| e.source_path.to_string_lossy().into_owned())
                .collect();
            let transaction = controller
                .apply(approved, tx.plan_generation, &cancel)?
                .transaction;
            Ok(Execution {
                completed: transaction.state == TransactionState::Applied
                    && transaction
                        .entries
                        .iter()
                        .all(|e| e.state == EntryState::Applied),
                transaction,
            })
        }
        RequestedAction::RetainForeign => {
            let transaction = controller.retain_foreign(&cancel)?;
            // A separate fresh ordinary Undo must verify and release ownership.
            Ok(Execution {
                completed: false,
                transaction,
            })
        }
        RequestedAction::Undo => {
            let result = controller.undo(&cancel)?;
            Ok(Execution {
                completed: matches!(result.result, RollbackResult::FullyRolledBack)
                    && result.transaction.is_fully_rolled_back(),
                transaction: result.transaction,
            })
        }
    }
}

/// Bounded private JSON/plan I/O for the operator CLI; no symlink/FIFO following.
pub fn read_document(path: &Path) -> io::Result<Vec<u8>> {
    let parent = path
        .parent()
        .ok_or_else(|| fail("missing document parent"))?;
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| fail("invalid document name"))?;
    read(&directory(parent)?, name)
}
pub fn write_document(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| fail("missing document parent"))?;
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| fail("invalid document name"))?;
    write_new(&directory(parent)?, name, bytes)
}

#[path = "restart_status.rs"]
mod restart_status;
pub use super::approval::GrantVerdict;
pub use restart_status::{
    EntryStatus, GrantStatus, NextStep, RestartState, RestartStatus, RetainedFile, inspect_grant,
    status,
};

#[cfg(test)]
#[path = "deployment_tests.rs"]
mod tests;
