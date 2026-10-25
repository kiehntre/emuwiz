//! Atomic publication with preservation of every displaced foreign file.
use super::*;

fn matches(path: &Path, identity: FileIdentity, sha: &str) -> bool {
    path_identity(path).is_ok_and(|found| found == identity)
        && fs::read(path).is_ok_and(|bytes| sha256_hex(&bytes) == sha)
}

pub(super) fn publish(
    staged: &Path,
    card: &Path,
    expected: FileIdentity,
    expected_sha: &str,
    new_sha: &str,
    changed: &mut bool,
    hook: Hook<'_>,
    after: Option<Ps2RestoreStep>,
) -> Result<(), Fail> {
    let incoming = path_identity(staged)?;
    if !matches(staged, incoming, new_sha) {
        return Err(Ps2PsuRestoreError::CardChanged.into());
    }
    rename_exchange(staged, card).map_err(|error| {
        if exchange_unsupported(&error) {
            Ps2PsuRestoreError::UnsupportedAtomicExchange
        } else {
            restore_error(error)
        }
    })?;
    *changed = true;
    if let Some(after) = after {
        step(hook, after)?;
    }
    if !matches(staged, expected, expected_sha) {
        // Never overwrite a later change during reversal. Keep both paths if
        // we cannot prove the live image is the one we just published.
        let displaced = path_identity(staged)?;
        let displaced_sha = sha256_hex(&fs::read(staged).map_err(restore_error)?);
        if !matches(card, incoming, new_sha) {
            return Err(Ps2PsuRestoreError::RecoveryRequired(format!(
                "Card changed during publication. Displaced contents retained at {}. No automatic reversal was attempted.", staged.display()
            )).into());
        }
        rename_exchange(staged, card).map_err(|error| Ps2PsuRestoreError::RecoveryRequired(format!(
            "Could not reverse publication: {error}. Displaced contents retained at {}. Preserve this file, the live card and the backup.", staged.display()
        )))?;
        if !matches(card, displaced, &displaced_sha) || !matches(staged, incoming, new_sha) {
            return Err(Ps2PsuRestoreError::RecoveryRequired(format!(
                "Files changed during reversal; both paths retained, including {}. Manual inspection is required.", staged.display()
            )).into());
        }
        if let Some(parent) = card.parent() {
            fs::File::open(parent).and_then(|directory| directory.sync_all()).map_err(|error| Ps2PsuRestoreError::RecoveryRequired(format!("Reversal directory sync failed: {error}. Preserve both paths, including {}", staged.display())))?;
        }
        *changed = false;
        return Err(Ps2PsuRestoreError::CardChanged.into());
    }
    if !matches(card, incoming, new_sha) {
        return Err(Ps2PsuRestoreError::RecoveryRequired(format!(
            "Published card changed; pre-publication contents retained at {}. Manual inspection is required.", staged.display()
        )).into());
    }
    Ok(())
}

pub(super) fn rollback(run: &mut Run<'_>, hook: Hook<'_>) -> Result<(), String> {
    let staged = run
        .journal
        .staged_path
        .as_ref()
        .ok_or("Missing displaced file path")?;
    let expected = run
        .journal
        .post_identity
        .ok_or("Missing publication identity")?;
    let expected_sha = run
        .journal
        .post_sha256
        .as_deref()
        .ok_or("Missing publication hash")?;
    hook(Ps2RestoreStep::BeforeRollbackWrite).map_err(|error| error.to_string())?;
    if !matches(&run.journal.card_path, expected, expected_sha) {
        return Err(format!(
            "Live card changed after publication; rollback refused. Displaced contents retained at {}",
            staged.display()
        ));
    }
    if !matches(
        staged,
        run.journal.original_identity,
        &run.journal.original_sha256,
    ) {
        return Err(format!(
            "Displaced original changed; rollback refused. Preserve {}",
            staged.display()
        ));
    }
    let target_sha = run.journal.original_sha256.clone();
    let mut changed = false;
    publish(
        staged,
        &run.journal.card_path,
        expected,
        expected_sha,
        &target_sha,
        &mut changed,
        &no_hook,
        None,
    )
    .map_err(|failure| match failure {
        Fail::Error(error) => error.to_string(),
        #[cfg(test)]
        Fail::Crash => "interrupted rollback".into(),
    })?;
    if let Some(parent) = run.journal.card_path.parent() {
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string())?;
    }
    run.renamed = false;
    remove_owned_temp(&run.journal);
    Ok(())
}
