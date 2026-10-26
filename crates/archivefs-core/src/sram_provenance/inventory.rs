use super::*;
use crate::emulator_environment::retroarch::{ProfileKind, ProfileRef, ProfileScope};
use crate::persistent_state_inventory::{
    self as inventory, PersistentStateInventory, PersistentStateRoot, PersistentStateType,
    StateEmulator,
};
use std::collections::BTreeMap;

/// The ordinary inventory stays provider-neutral. This projection adds a
/// separate, sealed, per-artifact origin fact and never fills game_identity.
pub fn inventory_with_sram_origins(roots: &[PersistentStateRoot]) -> PersistentStateInventory {
    let profile = ProfileRef {
        profile_kind: ProfileKind::Native,
        scope: ProfileScope::User,
    };
    let scope = discover_scope(profile);
    let mut roots = roots.to_vec();
    if let Ok(s) = &scope
        && !roots
            .iter()
            .any(|r| r.path == s.saves && r.emulator == StateEmulator::RetroArch)
    {
        roots.push(PersistentStateRoot::configured(
            StateEmulator::RetroArch,
            s.saves.clone(),
            Some(PersistentStateType::NativeSave),
        ));
    }
    let mut result = inventory::inventory_persistent_state(&roots);
    if let Ok(d) = DIAGNOSTIC.lock()
        && let Some(reason) = d.as_ref()
    {
        result.warnings.push(format!(
            "Last SRAM capture was unavailable: {reason}. Unattested saves stay unassigned."
        ));
    }
    let Ok(root) = catalogue_root() else {
        return result;
    };
    if !root.exists() {
        return result;
    }
    if root.join("coordination-uncertain").exists() {
        result.warnings.push("SRAM coordination needs review after an unobserved session. New association capture is disabled.".into());
    }
    match store::Store::open(&root, false).and_then(|s| s.receipts()) {
        Ok(receipts) => project(&mut result, receipts, scope.as_ref().ok()),
        Err(e) => result.warnings.push(format!(
            "SRAM associations need review: {e}. No origin associations were applied."
        )),
    }
    // A pending entry is never an association. It remains visible after a
    // crashed/interrupted Blue session until a later inventory review.
    if fs::read_dir(&root).is_ok_and(|mut entries| {
        entries.any(|e| e.is_ok_and(|e| e.path().extension().is_some_and(|s| s == "pending")))
    }) {
        result.warnings.push(
            "SRAM capture pending or interrupted; incomplete sessions establish no association."
                .into(),
        );
    }
    result
}
fn context_current(ctx: &Context, scope: Option<&Scope>) -> Result<(), String> {
    if scope != Some(&ctx.scope) {
        return Err("Selected RetroArch configuration/save scope changed or is unavailable".into());
    }
    use crate::game_identity::{IdentityKind, IdentityStatus};
    if ctx.cartridge_identity.kind != IdentityKind::LooseRomSha256
        || ctx.cartridge_identity.status != IdentityStatus::Verified
        || ctx.cartridge_identity.value.as_ref() != Some(&ctx.cartridge.sha256)
    {
        return Err("Verified Gold cartridge evidence unavailable".into());
    }
    current(&ctx.scope.configuration)?;
    current(&ctx.cartridge)?;
    current(&ctx.core)?;
    current(&ctx.executable)
}
pub(super) fn project(
    result: &mut PersistentStateInventory,
    receipts: Vec<Receipt>,
    scope: Option<&Scope>,
) {
    let mut by_path: BTreeMap<PathBuf, Vec<SramOrigin>> = BTreeMap::new();
    for receipt in receipts {
        if let Some(reason) = receipt.reason {
            result
                .warnings
                .push(format!("SRAM session {}: {reason}", receipt.session));
        }
        for mut origin in receipt.origins {
            let valid = (|| {
                if origin.context != receipt.context
                    || origin.session != receipt.session
                    || origin.state != OriginState::Associated
                {
                    return Err("Inconsistent capture receipt".into());
                }
                context_current(&origin.context, scope)?;
                current(&origin.artifact)
            })();
            if let Err(e) = valid {
                origin.state = OriginState::NeedsReview;
                origin.explanation = e;
            }
            by_path
                .entry(origin.artifact.path.clone())
                .or_default()
                .push(origin);
        }
    }
    for (path, origins) in by_path {
        // Multiple successful refreshes of the same context are normal. Only
        // exact current evidence can associate; different claimed contexts
        // for this artifact are an ownership conflict, including old receipts.
        let conflict = origins.iter().any(|o| o.context != origins[0].context);
        let mut selected = origins
            .iter()
            .find(|o| o.state == OriginState::Associated)
            .unwrap_or(&origins[0])
            .clone();
        if conflict {
            selected.state = OriginState::NeedsReview;
            selected.explanation =
                "Conflicting cartridge/core/scope origin records; no automatic association.".into();
        }
        let represented = result
            .records
            .iter()
            .any(|r| r.path == path && r.emulator == StateEmulator::RetroArch);
        let kind_conflict = result.records.iter().any(|r| {
            r.path == path
                && r.emulator == StateEmulator::RetroArch
                && !matches!(
                    r.state_type,
                    PersistentStateType::NativeSave | PersistentStateType::Unknown
                )
        });
        let identity_conflict = result
            .records
            .iter()
            .filter(|r| r.path == path && r.emulator == StateEmulator::RetroArch)
            .flat_map(|r| r.game_identity.iter())
            .any(|e| {
                e.kind == crate::game_identity::IdentityKind::LooseRomSha256
                    && e.status == crate::game_identity::IdentityStatus::Verified
                    && e.value
                        .as_ref()
                        .is_some_and(|v| v != &selected.context.cartridge.sha256)
            });
        if kind_conflict || identity_conflict {
            selected.state = OriginState::NeedsReview;
            selected.explanation="Conflicting inventory kind or cartridge identity metadata; no automatic association.".into();
        }
        let mut found = false;
        for record in result.records.iter_mut().filter(|r| {
            r.path == path
                && r.emulator == StateEmulator::RetroArch
                && matches!(
                    r.state_type,
                    PersistentStateType::NativeSave | PersistentStateType::Unknown
                )
        }) {
            found = true;
            let mut origin = selected.clone();
            if record.sha256.as_ref() != Some(&origin.artifact.sha256)
                || record.size_bytes != origin.artifact.stamp.size
            {
                origin.state = OriginState::NeedsReview;
                origin.explanation = "Save bytes differ from the captured artifact.".into();
            }
            if origin.state == OriginState::Associated
                && record.state_type == PersistentStateType::Unknown
            {
                record.state_type = PersistentStateType::NativeSave;
                record.provenance.push_str(
                    "; this artifact's SRAM kind was witnessed during the verified session",
                );
            }
            if origin.state == OriginState::NeedsReview {
                record
                    .warnings
                    .push(format!("SRAM origin needs review: {}", origin.explanation));
            }
            record.sram_origin = Some(origin);
        }
        // Generic inventory deliberately omits some unknown filenames. A
        // sealed, current SRAM witness may materialize its exact artifact;
        // neither the name nor its containing directory supplies identity.
        if !represented && selected.state == OriginState::Associated && result.records.len() < 4096
        {
            let root = PersistentStateRoot::configured(
                StateEmulator::RetroArch,
                path.clone(),
                Some(PersistentStateType::NativeSave),
            );
            let single = inventory::inventory_persistent_state(&[root]);
            if let Some(mut record) = single.records.into_iter().find(|r| {
                r.path == path
                    && r.state_type == PersistentStateType::NativeSave
                    && r.sha256.as_ref() == Some(&selected.artifact.sha256)
                    && r.size_bytes == selected.artifact.stamp.size
            }) && current(&selected.artifact).is_ok()
            {
                record.portability_class = inventory::PortabilityClass::DoNotTouch;
                record.provenance.push_str("; exact artifact materialized from a current SRAM witness; no restore authority");
                record.sram_origin = Some(selected.clone());
                result.records.push(record);
                found = true;
            }
        }
        if !found {
            result.warnings.push(format!("SRAM evidence needs review: {} is missing, moved, unreadable, or outside the selected inventory scope. No filename search was used.",path.display()));
        }
    }
}
