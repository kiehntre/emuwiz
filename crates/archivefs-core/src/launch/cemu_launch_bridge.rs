//! Launch-scoped structural identity; never a verified hash or DAT identity.
use crate::launch::cemu_readiness::CemuReadinessInspection;
use crate::launch::{
    CanonicalIdentityStatus, LaunchContainerKind, LaunchContentKind, LaunchContentRef,
    ResolvedIdentity,
};
use crate::wiiu_content_readiness::launch_selection::extracted_root_for_selection;
use std::path::Path;

/// Route the exact selected extracted base into the existing resolver. Preserve
/// conflicts and stronger identities, and refuse stale or differently bound facts.
pub fn inputs(
    selected: &Path,
    previous: &CanonicalIdentityStatus,
    inspection: &CemuReadinessInspection,
) -> Option<(CanonicalIdentityStatus, LaunchContentRef)> {
    let root = extracted_root_for_selection(selected)?;
    if root != inspection.content.source
        || !inspection.refusals.is_empty()
        || inspection.stamp.is_stale()
    {
        return None;
    }
    let id = inspection.content.identity.as_ref()?.title_id.as_ref()?;
    match previous {
        CanonicalIdentityStatus::Conflicting => return None,
        CanonicalIdentityStatus::Resolved(identity)
            if identity.platform_id != "WiiU" || !identity.game_key.eq_ignore_ascii_case(id) =>
        {
            return None;
        }
        _ => {}
    }
    // Compare the current structured field with the inspected one as well as
    // the stat stamp. Restored timestamps cannot substitute a different title.
    let current = crate::wiiu_content_readiness::project_wiiu_content_readiness(
        &root,
        crate::wiiu_content_readiness::WiiUKeyStoreEvidence::NotChecked,
        None,
    );
    if current != inspection.content {
        return None;
    }
    Some((CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "WiiU".into(), game_key: id.clone(),
    }), LaunchContentRef {
        kind: Some(LaunchContentKind::Executable),
        container: Some(LaunchContainerKind::PlainFile),
        resolved_path: Some(root), requires_mount: false,
        provenance: "Structurally identified extracted Wii U base: parsed meta.xml TITLE_ID and bounded Cafe RPX format. Not hash/DAT verification; Cemu preflight revalidates this exact selection.".into(),
    }))
}

/// A worker-observed launch-only selection. UI projection is pure and expires;
/// execution must call `inputs` again against live files before using the backend.
#[derive(Clone, Debug)]
pub struct Selection {
    selected: std::path::PathBuf,
    identity: CanonicalIdentityStatus,
    content: LaunchContentRef,
    observed: std::time::Instant,
}
impl Selection {
    pub fn inspect(selected: &Path, inspection: &CemuReadinessInspection) -> Option<Self> {
        let (identity, content) = inputs(selected, &CanonicalIdentityStatus::Unknown, inspection)?;
        Some(Self {
            selected: selected.into(),
            identity,
            content,
            observed: std::time::Instant::now(),
        })
    }
    pub fn expired(&self) -> bool {
        self.observed.elapsed() > std::time::Duration::from_secs(30)
    }
    pub fn project(
        &self,
        selected: &Path,
        previous: &CanonicalIdentityStatus,
    ) -> Option<(CanonicalIdentityStatus, LaunchContentRef)> {
        if self.expired() || self.selected != selected {
            return None;
        }
        match (previous, &self.identity) {
            (CanonicalIdentityStatus::Unknown, _) => {}
            (CanonicalIdentityStatus::Resolved(old), CanonicalIdentityStatus::Resolved(new))
                if old.platform_id == new.platform_id
                    && old.game_key.eq_ignore_ascii_case(&new.game_key) => {}
            _ => return None,
        }
        Some((self.identity.clone(), self.content.clone()))
    }
}
