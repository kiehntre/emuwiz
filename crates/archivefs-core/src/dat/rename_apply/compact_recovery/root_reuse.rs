//! Explicit post-release lifecycle boundary. No registration/container/grant is
//! moved, reused or deleted. Successors share admission, not mutation authority.
use super::*;
const CLOSED: &str = "settled-v1.json";
pub(super) const FORMAT_NEXT: &str = "emuwiz-r2-successor-v1";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Lineage {
    pub(super) origin: PathBuf,
    predecessor: PathBuf,
    predecessor_id: String,
    predecessor_binding: [u8; 32],
}
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Settlement {
    format: String,
    state: PathBuf,
    state_id: (u64, u64),
    configuration: [u8; 32],
    registration: [u8; 32],
    image: [u8; 32],
    prefix: Vec<u8>,
    transaction: String,
}
#[derive(Debug, Serialize)]
pub struct ReuseReview {
    pub confirmation: String,
    pub predecessor: String,
    pub next_state: PathBuf,
    pub effect: String,
}
#[derive(Serialize)]
pub struct HistoricalTransaction {
    pub state: PathBuf,
    pub transaction: RenameTransaction,
    pub historical_settlement: bool,
    pub retained: Vec<RetainedFile>,
}
pub(super) fn exists(path: &Path) -> io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}
fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
fn config_digest(c: &Configuration) -> io::Result<[u8; 32]> {
    let mut budget = u64::MAX;
    config_digest_budget(c, &mut budget)
}
fn config_digest_budget(c: &Configuration, budget: &mut u64) -> io::Result<[u8; 32]> {
    Ok(digest(&read_budget(
        &directory(&c.state)?,
        "configuration.json",
        budget,
    )?))
}
fn registration(c: &Configuration) -> io::Result<(Registration, Authority)> {
    let mut budget = u64::MAX;
    registration_budget(c, &mut budget)
}
fn registration_budget(
    c: &Configuration,
    budget: &mut u64,
) -> io::Result<(Registration, Authority)> {
    let r: Registration = serde_json::from_slice(&read_budget(
        &directory(&c.state)?,
        "registration.json",
        budget,
    )?)
    .map_err(fail)?;
    let b = Authority::decode_legacy(&r.original, 1024 * 1024).map_err(protocol)?;
    if r.ticket.root != c.root
        || r.ticket.root_id != c.root_id
        || r.ticket.binding != b.binding()
        || !shared_engine::selected(&b.transaction)
    {
        return Err(fail("reuse registration differs from root/authority"));
    }
    Bridge::supported(&b)?;
    Bridge::confined(&c.root, &b)?;
    Ok((r, b))
}
pub(super) fn validate_lineage(
    c: &Configuration,
    records: &[admission::Successor],
) -> io::Result<()> {
    let mut budget = u64::MAX;
    validate_lineage_budget(c, records, &mut budget)
}
fn validate_lineage_budget(
    c: &Configuration,
    records: &[admission::Successor],
    budget: &mut u64,
) -> io::Result<()> {
    if let Some(l) = &c.lineage {
        let r=records.iter().find(|r|r.predecessor==l.predecessor_id).ok_or_else(||fail("successor activation incomplete; retry reuse from predecessor with fresh preview"))?;
        let p = load_config_budget(&l.predecessor, budget)?;
        let (registration, base) = registration_budget(&p, budget)?;
        let store = Store::inspect(&registration.ticket, &base, CAP)?;
        let bytes = read_budget(&store.directory, CLOSED, budget)?;
        if r.binding != l.predecessor_binding
            || base.binding() != l.predecessor_binding
            || base.transaction_id() != l.predecessor_id
            || r.state != c.state
            || r.state_id != c.state_id
            || r.configuration != config_digest_budget(c, budget)?
            || r.settlement != digest(&bytes)
            || p.lineage.as_ref().map(|v| &v.origin).unwrap_or(&p.state) != &l.origin
        {
            return Err(fail(
                "successor lineage, configuration or settlement changed",
            ));
        }
    }
    Ok(())
}
/// A tiny immutable metadata publication. Interrupted temporary records are
/// preserved, not authoritative, and never treated as a completed transition.
fn pending_name(name: &str) -> bool {
    name.strip_prefix(".r2-pending-")
        .is_some_and(|n| n.len() == 32 && n.bytes().all(|b| b.is_ascii_hexdigit()))
}
fn publish(dir: &File, name: &str, bytes: &[u8], boundary: &str) -> io::Result<()> {
    if exists(&at(dir, name))? {
        if read(dir, name)? != bytes {
            return Err(fail("lifecycle receipt conflicts; retain for review"));
        }
        let f = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(at(dir, name))?;
        f.sync_all()?;
        return dir.sync_all();
    }
    let mut nonce = [0u8; 16];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut nonce).map_err(fail)?;
    let tmp = format!(".r2-pending-{}", hex(&nonce));
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(at(dir, &tmp))?;
    edge(&format!("{boundary}-created"));
    f.write_all(bytes)?;
    edge(&format!("{boundary}-written"));
    f.sync_all()?;
    edge(&format!("{boundary}-synced"));
    dir.sync_all()?;
    edge(boundary);
    let from = std::ffi::CString::new(tmp).map_err(fail)?;
    let to = std::ffi::CString::new(name).map_err(fail)?;
    if unsafe {
        libc::renameat2(
            dir.as_raw_fd(),
            from.as_ptr(),
            dir.as_raw_fd(),
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    edge(&format!("{boundary}-renamed"));
    dir.sync_all()
}
fn retained(
    c: &Configuration,
    tx: &RenameTransaction,
    budget: &mut u64,
) -> io::Result<Vec<RetainedFile>> {
    let mut out = Vec::new();
    for (i, e) in tx.entries.iter().enumerate() {
        let Some(v) = contract::evidence(tx, i)? else {
            continue;
        };
        if v.retention.is_none() {
            continue;
        }
        if v.phase != contract::Phase::RetainedForeign || v.action != contract::Action::Undo {
            return Err(fail("unsettled historical retention"));
        }
        let foreign = v
            .held_identity
            .as_ref()
            .ok_or_else(|| fail("retained identity missing"))?;
        let (held, salvage) = contract::paths(&c.root, tx, i)?;
        let anchor = contract::anchor::path(&c.root, tx, i)?;
        directory(
            held.parent()
                .ok_or_else(|| fail("holding parent missing"))?,
        )?;
        if crate::dat::rename_apply::identity::capture_identity_bounded(&held, budget)? != *foreign
            || crate::dat::rename_apply::identity::capture_identity_bounded(&anchor, budget)?
                != e.identity
            || exists(&salvage)?
        {
            return Err(fail(
                "retained historical object or continuity anchor changed; review required",
            ));
        }
        out.push(RetainedFile {
            path: held,
            identity: foreign.clone(),
            original_anchor: anchor,
            signed_grant_nonce: v.retention.unwrap().nonce,
            sha256: hex(&foreign
                .freshness
                .as_ref()
                .ok_or_else(|| fail("retained digest missing"))?
                .sha256),
        });
    }
    Ok(out)
}
fn expected(
    c: &Configuration,
    r: &Registration,
    base: &Authority,
    store: &Store,
    budget: &mut u64,
) -> io::Result<(Settlement, RenameTransaction)> {
    *budget = budget
        .checked_sub(store.layout.length)
        .ok_or_else(|| fail("historical scan budget exceeded"))?;
    let scan = store.scan(base)?;
    let observation = materialize(base, &scan.stream, usize::MAX).map_err(protocol)?;
    let tx = scan.runtime.restore(base, &scan.stream)?;
    if scan.pending.is_some() || !observation.rollback_recorded || !tx.is_fully_rolled_back() {
        return Err(fail("previous transaction not completely settled"));
    }
    *budget = budget
        .checked_sub(store.layout.length)
        .ok_or_else(|| fail("historical image budget exceeded"))?;
    let registration_bytes = read_budget(&directory(&c.state)?, "registration.json", budget)?;
    let current: Registration = serde_json::from_slice(&registration_bytes).map_err(fail)?;
    if serde_json::to_vec(&current).map_err(fail)? != serde_json::to_vec(r).map_err(fail)? {
        return Err(fail("registration changed during settlement"));
    }
    let record = Settlement {
        format: "emuwiz-r2-released-settlement-v1".into(),
        state: c.state.clone(),
        state_id: c.state_id,
        configuration: config_digest_budget(c, budget)?,
        registration: digest(&registration_bytes),
        image: digest(&approval::container_bytes(store)?),
        prefix: scan.cursor,
        transaction: base.transaction_id().into(),
    };
    Ok((record, tx))
}
fn closed(
    c: &Configuration,
    r: &Registration,
    base: &Authority,
    store: &Store,
    budget: &mut u64,
) -> io::Result<(RenameTransaction, Vec<RetainedFile>)> {
    let bytes = read_budget(&store.directory, CLOSED, budget)?;
    let record: Settlement = serde_json::from_slice(&bytes).map_err(fail)?;
    for entry in std::fs::read_dir(at(&store.directory, "."))? {
        let name = entry?.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| fail("invalid settlement preparation name"))?;
        if pending_name(name) {
            let partial = read_budget(&store.directory, name, budget)?;
            if !bytes.starts_with(&partial) {
                return Err(fail("conflicting settlement preparation; review required"));
            }
        } else if name.starts_with(".r2-pending-") {
            return Err(fail("unknown settlement preparation; review required"));
        }
    }
    let (wanted, tx) = expected(c, r, base, store, budget)?;
    if record != wanted {
        return Err(fail("historical settlement or container evidence differs"));
    }
    let proofs = tx
        .entries
        .iter()
        .filter(|e| {
            e.unknown
                .get(contract::KEY)
                .is_some_and(|v| v.get("retention").is_some())
        })
        .count() as u64;
    *budget = budget
        .checked_sub(proofs * 2 * 16 * 1024)
        .ok_or_else(|| fail("retention receipt budget exceeded"))?;
    approval::verify_retention_receipts(&c.verifier, store, base, &tx)?;
    let held = retained(c, &tx, budget)?;
    Ok((tx, held))
}
fn closed_unlimited(
    c: &Configuration,
    r: &Registration,
    base: &Authority,
    store: &Store,
) -> io::Result<(RenameTransaction, Vec<RetainedFile>)> {
    let mut budget = u64::MAX;
    closed(c, r, base, store, &mut budget)
}
fn current_settled(
    c: &Configuration,
    r: &Registration,
    base: &Authority,
    store: &Store,
    guard: &admission::Guard,
) -> io::Result<(Settlement, RenameTransaction)> {
    guard.require_released(base)?;
    let mut budget = u64::MAX;
    let (record, tx) = expected(c, r, base, store, &mut budget)?;
    if exists(&at(&store.directory, CLOSED))? {
        closed_unlimited(c, r, base, store)?;
    } else {
        contract::verify_records(&tx)?;
        approval::verify_retention_receipts(&c.verifier, store, base, &tx)?;
        for e in &tx.entries {
            if capture_identity(&e.source_path)? != e.identity || exists(&e.destination_path)? {
                return Err(fail("original not restored at settlement boundary"));
            }
        }
    }
    Ok((record, tx))
}
pub(super) fn verify_predecessors(c: &Configuration, guard: &admission::Guard) -> io::Result<()> {
    walk_predecessors(c, &guard.successors()?, |base| guard.require_released(base))
}
pub(super) fn verify_predecessors_read(
    c: &Configuration,
    inspection: &admission::Inspection,
) -> io::Result<()> {
    walk_predecessors(c, &inspection.successors, |base| {
        if !inspection
            .released_owners
            .contains(&(base.transaction_id().into(), base.binding().to_vec()))
        {
            return Err(fail("predecessor ownership not durably released"));
        }
        Ok(())
    })
}
fn walk_predecessors(
    c: &Configuration,
    records: &[admission::Successor],
    mut released: impl FnMut(&Authority) -> io::Result<()>,
) -> io::Result<()> {
    let mut cursor = load_config(&c.state)?;
    let mut visited = std::collections::BTreeSet::new();
    let mut ids = std::collections::BTreeSet::new();
    let mut generations = std::collections::BTreeSet::new();
    // Include only the current, explicitly registered generation; it will be
    // checked by the normal current-state reader/executor, not called settled.
    if exists(&c.state.join("registration.json"))? {
        let (r, b) = registration(c)?;
        ids.insert(b.transaction_id().to_owned());
        if let Some(generation) = r.ticket.generation {
            generations.insert(generation_name(Some(&generation)));
        }
    }
    while let Some(l) = &cursor.lineage {
        if !visited.insert(cursor.state.clone()) {
            return Err(fail("cyclic lifecycle history"));
        }
        validate_lineage(&cursor, records)?;
        let previous = load_config(&l.predecessor)?;
        let (r, b) = registration(&previous)?;
        released(&b)?;
        let store = Store::inspect(&r.ticket, &b, CAP)?;
        closed_unlimited(&previous, &r, &b, &store)?;
        ids.insert(b.transaction_id().to_owned());
        if let Some(generation) = r.ticket.generation {
            generations.insert(generation_name(Some(&generation)));
        }
        cursor = previous;
    }
    if c.lineage.is_some() {
        for entry in std::fs::read_dir(c.root.join(NAMESPACE))? {
            let name = entry?.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| fail("invalid generation name"))?;
            if name != LEAF && name != CLOSED && !pending_name(name) && name != "generations" {
                return Err(fail("unknown recovery generation; retain for review"));
            }
        }
        let generation_root = c.root.join(NAMESPACE).join("generations");
        if exists(&generation_root)? {
            directory(&generation_root)?;
            for entry in std::fs::read_dir(generation_root)? {
                let name = entry?.file_name();
                let name = name
                    .to_str()
                    .ok_or_else(|| fail("invalid generation entry"))?;
                if !generations.contains(name) {
                    return Err(fail("unknown recovery generation; retain for review"));
                }
            }
        } else if !generations.is_empty() {
            return Err(fail("generation namespace missing"));
        }
        for entry in std::fs::read_dir(c.root.join(".emuwiz-r2-holding"))? {
            let name = entry?.file_name();
            let name = name.to_str().ok_or_else(|| fail("invalid holding name"))?;
            if name != BINDING && !ids.contains(name) {
                return Err(fail(
                    "unknown or unresolved holding evidence; retain for review",
                ));
            }
        }
    }
    Ok(())
}
fn target(c: &Configuration, next: &Path) -> io::Result<Option<(u64, u64)>> {
    if !next.is_absolute()
        || next.starts_with(&c.root)
        || c.root.starts_with(next)
        || next.starts_with(&c.state)
        || c.state.starts_with(next)
    {
        return Err(fail("new separate private state path required"));
    }
    let origin = c
        .lineage
        .as_ref()
        .map(|lineage| &lineage.origin)
        .unwrap_or(&c.state);
    if next.parent() != origin.parent() {
        return Err(fail(
            "successor state must be a separate sibling of the original private state",
        ));
    }
    let parent = directory(
        next.parent()
            .ok_or_else(|| fail("missing successor parent"))?,
    )?;
    if parent.metadata()?.dev() != directory(&c.root)?.metadata()?.dev() {
        return Err(fail("successor must share local eligible filesystem"));
    }
    if exists(next)? {
        Ok(Some(identity(&directory(next)?)?))
    } else {
        Ok(None)
    }
}
fn review_locked(
    c: &Configuration,
    next: &Path,
    guard: &admission::Guard,
) -> io::Result<(ReuseReview, Registration, Authority, Store, Settlement)> {
    validate_lineage(c, &guard.successors()?)?;
    verify_predecessors(c, guard)?;
    let (r, b) = registration(c)?;
    let store = Store::inspect(&r.ticket, &b, CAP)?;
    let (seal, _) = current_settled(c, &r, &b, &store, guard)?;
    let child = target(c, next)?;
    let confirmation =
        hash(&serde_json::to_vec(&("r2-explicit-successor-v1", &seal, next, child)).map_err(fail)?);
    Ok((ReuseReview {confirmation,predecessor:b.transaction_id().into(),next_state:next.into(),effect:"Freeze verified released Undo as historical settlement; retain all journals, grants and foreign objects. Create a separate transaction registration on the SAME root. No Apply or enrollment is authorised.".into()},r,b,store,seal))
}
pub fn preview_reuse(state: &Path, next: &Path) -> io::Result<ReuseReview> {
    let c = load(state)?;
    let guard = c.authority.acquire()?;
    let (review, _, _, _, _) = review_locked(&c, next, &guard)?;
    Ok(review)
}
pub fn reuse(state: &Path, next: &Path, confirmation: &str, enable: bool) -> io::Result<()> {
    enabled(enable)?;
    let c = load(state)?;
    let mut guard = c.authority.acquire()?;
    let (review, r, b, store, seal) = review_locked(&c, next, &guard)?;
    let existing = guard
        .successors()?
        .into_iter()
        .find(|v| v.predecessor == b.transaction_id());
    if let Some(existing) = existing {
        if existing.state != next {
            return Err(fail("predecessor already has a different successor"));
        }
        if confirmation != review.confirmation && confirmation != existing.reviewed_confirmation {
            return Err(fail("reuse review stale or substituted"));
        }
        let child = load_config(next)?;
        validate_lineage(&child, &guard.successors()?)?;
        return Ok(());
    }
    if confirmation != review.confirmation {
        return Err(fail("reuse review stale or substituted"));
    }
    guard.require_idle()?;
    contract::capability::probe(&c.root)?;
    capacity(&directory(&c.root)?)?;
    let bytes = serde_json::to_vec(&seal).map_err(fail)?;
    publish(
        &store.directory,
        CLOSED,
        &bytes,
        "reuse-before-settlement-publish",
    )?;
    edge("reuse-after-settlement-publish");
    closed_unlimited(&c, &r, &b, &store)?;
    let dir = if exists(next)? {
        directory(next)?
    } else {
        create_directory(next)?
    };
    edge("reuse-after-state-create");
    capacity(&dir)?;
    let child = Configuration {
        format: FORMAT_NEXT.into(),
        root: c.root.clone(),
        root_id: c.root_id,
        state: next.into(),
        state_id: identity(&dir)?,
        operator: c.operator.clone(),
        authority: c.authority.clone(),
        verifier: c.verifier.clone(),
        lineage: Some(Lineage {
            origin: c
                .lineage
                .as_ref()
                .map(|l| l.origin.clone())
                .unwrap_or_else(|| c.state.clone()),
            predecessor: c.state.clone(),
            predecessor_id: b.transaction_id().into(),
            predecessor_binding: b.binding(),
        }),
    };
    let config = serde_json::to_vec(&child).map_err(fail)?;
    // A fresh private empty/prepared directory can be explicitly reviewed after
    // interruption. Never adopt foreign nonempty data or replace a configuration.
    for entry in std::fs::read_dir(at(&dir, "."))? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| fail("invalid prepared state name"))?;
        if name != "configuration.json" && !pending_name(name) {
            return Err(fail(
                "successor state contains unrelated evidence; review required",
            ));
        }
        let held = read(&dir, name)?;
        if pending_name(name) && !config.starts_with(&held) {
            return Err(fail("conflicting successor preparation; review required"));
        }
    }
    publish(
        &dir,
        "configuration.json",
        &config,
        "reuse-before-config-publish",
    )?;
    edge("reuse-after-config-publish");
    if identity(&directory(next)?)? != identity(&dir)? || config_digest(&child)? != digest(&config)
    {
        return Err(fail("prepared successor state changed"));
    }
    // Atomic SQLite activation is the only event that enables successor use.
    // Until then load(next) refuses; retry the predecessor reuse command.
    let record = admission::Successor {
        predecessor: b.transaction_id().into(),
        binding: b.binding(),
        state: next.into(),
        state_id: child.state_id,
        configuration: digest(&config),
        settlement: digest(&bytes),
        reviewed_confirmation: confirmation.into(),
    };
    edge("reuse-before-successor-commit");
    guard.publish_successor(&b, &record)?;
    edge("reuse-after-successor-commit");
    validate_lineage(&child, &guard.successors()?)?;
    Ok(())
}
pub fn history(state: &Path) -> io::Result<Vec<HistoricalTransaction>> {
    let c = load(state)?;
    let guard = c.authority.acquire()?;
    verify_predecessors(&c, &guard)?;
    let mut current = c;
    let mut result = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    loop {
        if !seen.insert(current.state.clone()) {
            return Err(fail("cyclic lifecycle history"));
        }
        if exists(&current.state.join("registration.json"))? {
            let (r, b) = registration(&current)?;
            let store = Store::inspect(&r.ticket, &b, CAP)?;
            if exists(&at(&store.directory, CLOSED))? {
                guard.require_released(&b)?;
                let (tx, retained) = closed_unlimited(&current, &r, &b, &store)?;
                result.push(HistoricalTransaction {
                    state: current.state.clone(),
                    transaction: tx,
                    historical_settlement: true,
                    retained,
                });
            }
        }
        match current.lineage {
            None => break,
            Some(l) => current = load_config(&l.predecessor)?,
        }
    }
    Ok(result)
}
pub(super) fn historical_read(
    store: &Store,
    base: &Authority,
    tx: &RenameTransaction,
    prefix: &[u8],
    budget: &mut u64,
) -> Option<io::Result<()>> {
    match exists(&at(&store.directory, CLOSED)) {
        Ok(false) => return None,
        Err(e) => return Some(Err(e)),
        _ => (),
    }
    Some((|| {
        let receipt: Settlement =
            serde_json::from_slice(&read_budget(&store.directory, CLOSED, budget)?)
                .map_err(fail)?;
        let c = load_config_budget(&receipt.state, budget)?;
        let (r, b) = registration_budget(&c, budget)?;
        if b.binding() != base.binding()
            || serde_json::to_vec(&r.ticket).map_err(fail)?
                != serde_json::to_vec(&store.ticket).map_err(fail)?
        {
            return Err(fail("historical receipt belongs to another registration"));
        }
        let (ownership, _hold) = c.authority.inspect(Some(base))?;
        validate_lineage_budget(&c, &ownership.successors, budget)?;
        if ownership.owner != Some((base.binding().to_vec(), true)) || ownership.owner_claims != 0 {
            return Err(fail("historical settlement ownership not released"));
        }
        let (restored, _) = closed(&c, &r, base, store, budget)?;
        if restored != *tx || receipt.prefix != prefix {
            return Err(fail("historical snapshot differs"));
        }
        Ok(())
    })())
}

pub(super) fn closed_status(
    c: &Configuration,
    r: &Registration,
    base: &Authority,
    store: &Store,
    ownership: &admission::Inspection,
) -> Option<io::Result<RestartStatus>> {
    match exists(&at(&store.directory, CLOSED)) {
        Ok(false) => return None,
        Err(e) => return Some(Err(e)),
        _ => (),
    }
    Some((|| {
        validate_lineage(c, &ownership.successors)?;
        if ownership.owner != Some((base.binding().to_vec(), true)) || ownership.owner_claims != 0 {
            return Err(fail("historical ownership not released"));
        }
        let (tx, retained) = closed_unlimited(c, r, base, store)?;
        let mut entries = Vec::new();
        for (i, e) in tx.entries.iter().enumerate() {
            let evidence = contract::evidence(&tx, i)?;
            let path = contract::paths(&c.root, &tx, i)?.0;
            entries.push(EntryStatus { index:i,source:e.source_path.clone(),destination:e.destination_path.clone(),state:e.state,
                action:evidence.as_ref().map(|_|"undo".into()),phase:evidence.as_ref().map(|v|format!("{:?}",v.phase)),
                verdict:"Historical verified settlement; current library names may belong to a later transaction".into(),
                retained_foreign:retained.iter().find(|r|r.path==path).cloned() });
        }
        Ok(RestartStatus {state:RestartState::Settled,next:NextStep::None,reasons:vec!["Verified historical Undo and released ownership. No old grant can act. Retained objects and their consumed signed receipts remain verified; current library pathnames are not asserted as an old transaction's current state.".into()],transaction_id:Some(tx.transaction_id),checkpoint_pending:false,ownership:"released".into(),entries})
    })())
}
