//! Shared activation state, ordering, and conflict projection for ordinary mods.
//!
//! This module deliberately does not install files or rewrite emulator
//! configuration. Existing adapter-specific preview/transaction plans remain
//! the authority for those operations. It gives GUI-v2 and callers one small,
//! serializable model for deciding which plans may be enabled together.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::patch_manager::{PreviewAdapter, SharedTransactionPlan};

pub const MAX_STACK_LAYERS: usize = 512;
pub const MAX_PATHS_PER_LAYER: usize = 4096;
pub const MAX_REPORTED_OVERLAPS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModProvenance {
    pub source: String,
    pub content_sha256: Option<String>,
    pub receipt_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchChainStage {
    pub chain_id: String,
    pub expected_input_sha256: Option<String>,
    pub produced_output_sha256: Option<String>,
    pub explicit_order: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModLayer {
    pub mod_id: String,
    pub package_id: String,
    pub game_identity: String,
    pub platform: String,
    pub emulator: Option<String>,
    pub mod_type: String,
    pub provenance: ModProvenance,
    pub enabled: bool,
    pub requested_order: Option<u32>,
    pub effective_order: Option<u32>,
    pub affected_paths: Vec<PathBuf>,
    pub derived_output: Option<PathBuf>,
    pub exclusive_group: Option<String>,
    pub patch_chain: Option<PatchChainStage>,
    pub destination_fingerprint: Option<String>,
    pub current_destination_fingerprint: Option<String>,
    pub transaction_id: Option<String>,
    pub adapter: Option<PreviewAdapter>,
}

impl ModLayer {
    pub fn from_shared_transaction(
        mod_id: impl Into<String>,
        package_id: impl Into<String>,
        game_identity: impl Into<String>,
        platform: impl Into<String>,
        mod_type: impl Into<String>,
        provenance: ModProvenance,
        plan: &SharedTransactionPlan,
    ) -> Result<Self, String> {
        if plan.entries.is_empty() {
            return Err("the transaction plan contains no affected files".into());
        }
        let paths = plan
            .entries
            .iter()
            .map(|entry| PathBuf::from(&entry.destination_relative_path.display))
            .collect::<Vec<_>>();
        Ok(Self {
            mod_id: mod_id.into(),
            package_id: package_id.into(),
            game_identity: game_identity.into(),
            platform: platform.into(),
            emulator: Some(format!("{:?}", plan.context.adapter)),
            mod_type: mod_type.into(),
            provenance,
            enabled: false,
            requested_order: None,
            effective_order: None,
            affected_paths: paths,
            derived_output: None,
            exclusive_group: None,
            patch_chain: None,
            destination_fingerprint: None,
            current_destination_fingerprint: None,
            transaction_id: Some(plan.plan_id.clone()),
            adapter: Some(plan.context.adapter),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    Informational,
    PathOverlap,
    DerivedOutputOverlap,
    EmulatorReplacementOverlap,
    MutuallyExclusive,
    DuplicateIdentity,
    IncompatiblePatchChain,
    SourceHashMismatch,
    DestinationChanged,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictClass {
    Informational,
    OrderingRequired,
    NeedsReview,
    UnsafeRefuse,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModConflict {
    pub left_mod_id: String,
    pub right_mod_id: Option<String>,
    pub kind: ConflictKind,
    pub class: ConflictClass,
    pub paths: Vec<PathBuf>,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationOperation {
    Enable,
    Disable,
    Reorder,
    Group,
    Rollback,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModStack {
    pub game_identity: String,
    pub layers: Vec<ModLayer>,
    pub generation: u64,
}

impl ModStack {
    pub fn new(game_identity: impl Into<String>) -> Self {
        Self {
            game_identity: game_identity.into(),
            layers: Vec::new(),
            generation: 0,
        }
    }

    pub fn preview_enable(&self, layer: ModLayer) -> ActivationPlan {
        let mut layers = self.layers.clone();
        if let Some(existing) = layers
            .iter_mut()
            .find(|existing| existing.mod_id == layer.mod_id && !existing.enabled)
        {
            existing.enabled = true;
        } else {
            layers.push(layer);
        }
        plan(self, ActivationOperation::Enable, layers)
    }

    pub fn preview_disable(&self, mod_id: &str) -> ActivationPlan {
        let layers = self
            .layers
            .iter()
            .cloned()
            .map(|mut layer| {
                if layer.mod_id == mod_id {
                    layer.enabled = false;
                }
                layer
            })
            .collect();
        plan(self, ActivationOperation::Disable, layers)
    }

    pub fn preview_reorder(&self, ordered_ids: &[String]) -> ActivationPlan {
        let mut layers = self.layers.clone();
        for (order, id) in ordered_ids.iter().enumerate() {
            if let Some(layer) = layers.iter_mut().find(|layer| &layer.mod_id == id) {
                layer.requested_order = Some(order as u32);
            }
        }
        plan(self, ActivationOperation::Reorder, layers)
    }

    pub fn preview_group(&self, ids: &[String], enabled: bool) -> ActivationPlan {
        let wanted = ids.iter().collect::<BTreeSet<_>>();
        let layers = self
            .layers
            .iter()
            .cloned()
            .map(|mut layer| {
                if wanted.contains(&layer.mod_id) {
                    layer.enabled = enabled;
                }
                layer
            })
            .collect();
        plan(self, ActivationOperation::Group, layers)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivationPlan {
    pub operation: ActivationOperation,
    pub previous_stack: ModStack,
    pub resulting_stack: ModStack,
    pub conflicts: Vec<ModConflict>,
    pub warnings: Vec<String>,
    pub reversible: bool,
    pub refused: bool,
}

impl ActivationPlan {
    pub fn can_apply(&self) -> bool {
        !self.refused
    }

    pub fn receipt(
        &self,
        transaction_ids: Vec<String>,
        timestamp_unix_seconds: u64,
    ) -> ActivationReceipt {
        ActivationReceipt {
            previous_stack: self.previous_stack.clone(),
            resulting_stack: self.resulting_stack.clone(),
            operation: self.operation,
            conflicts: self.conflicts.clone(),
            transaction_ids,
            timestamp_unix_seconds,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivationReceipt {
    pub previous_stack: ModStack,
    pub resulting_stack: ModStack,
    pub operation: ActivationOperation,
    pub conflicts: Vec<ModConflict>,
    pub transaction_ids: Vec<String>,
    pub timestamp_unix_seconds: u64,
}

impl ActivationReceipt {
    pub fn rollback_plan(&self) -> ActivationPlan {
        plan(
            &self.resulting_stack,
            ActivationOperation::Rollback,
            self.previous_stack.layers.clone(),
        )
    }
}

fn plan(
    previous: &ModStack,
    operation: ActivationOperation,
    mut layers: Vec<ModLayer>,
) -> ActivationPlan {
    let mut conflicts = Vec::new();
    let mut warnings = Vec::new();
    let explicitly_ordered = layers
        .iter()
        .map(|layer| {
            (
                layer.mod_id.clone(),
                layer.requested_order.is_some()
                    || layer
                        .patch_chain
                        .as_ref()
                        .is_some_and(|chain| chain.explicit_order.is_some()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    if layers.len() > MAX_STACK_LAYERS {
        conflicts.push(ModConflict {
            left_mod_id: "stack".into(),
            right_mod_id: None,
            kind: ConflictKind::Unknown,
            class: ConflictClass::UnsafeRefuse,
            paths: Vec::new(),
            detail: format!("mod stack exceeds the {} layer limit", MAX_STACK_LAYERS),
        });
    }

    layers.sort_by(|left, right| {
        effective_requested_order(left)
            .unwrap_or(u32::MAX)
            .cmp(&effective_requested_order(right).unwrap_or(u32::MAX))
            .then_with(|| left.mod_id.cmp(&right.mod_id))
    });
    for (index, layer) in layers.iter_mut().enumerate() {
        layer.requested_order = layer.requested_order.or(Some(index as u32));
        layer.effective_order = Some(index as u32);
    }

    let mut identities = BTreeMap::<&str, &str>::new();
    for layer in &layers {
        if let Some(other) = identities.insert(&layer.mod_id, &layer.package_id) {
            conflicts.push(ModConflict {
                left_mod_id: layer.mod_id.clone(),
                right_mod_id: Some(layer.mod_id.clone()),
                kind: ConflictKind::DuplicateIdentity,
                class: ConflictClass::UnsafeRefuse,
                paths: Vec::new(),
                detail: format!(
                    "mod identity is duplicated (packages {other} and {})",
                    layer.package_id
                ),
            });
        }
        if layer.destination_fingerprint.is_some()
            && layer.destination_fingerprint != layer.current_destination_fingerprint
        {
            conflicts.push(ModConflict {
                left_mod_id: layer.mod_id.clone(),
                right_mod_id: None,
                kind: ConflictKind::DestinationChanged,
                class: ConflictClass::UnsafeRefuse,
                paths: Vec::new(),
                detail: "destination changed since the preview".into(),
            });
        }
        if layer.affected_paths.len() > MAX_PATHS_PER_LAYER {
            conflicts.push(ModConflict {
                left_mod_id: layer.mod_id.clone(),
                right_mod_id: None,
                kind: ConflictKind::Unknown,
                class: ConflictClass::UnsafeRefuse,
                paths: Vec::new(),
                detail: "affected path list exceeds the safety bound".into(),
            });
        }
        if layer.enabled {
            for path in &layer.affected_paths {
                if normalize_path(path).is_none() {
                    conflicts.push(ModConflict {
                        left_mod_id: layer.mod_id.clone(),
                        right_mod_id: None,
                        kind: ConflictKind::Unknown,
                        class: ConflictClass::UnsafeRefuse,
                        paths: vec![path.clone()],
                        detail: "affected path is not a safe relative path".into(),
                    });
                }
            }
        }
    }

    let enabled = layers
        .iter()
        .filter(|layer| layer.enabled)
        .collect::<Vec<_>>();
    for (index, left) in enabled.iter().enumerate() {
        for right in enabled.iter().skip(index + 1) {
            if left.game_identity != right.game_identity || left.platform != right.platform {
                conflicts.push(ModConflict {
                    left_mod_id: left.mod_id.clone(),
                    right_mod_id: Some(right.mod_id.clone()),
                    kind: ConflictKind::Unknown,
                    class: ConflictClass::UnsafeRefuse,
                    paths: Vec::new(),
                    detail: "mods target different verified game/platform identities".into(),
                });
                continue;
            }
            if left.exclusive_group.is_some() && left.exclusive_group == right.exclusive_group {
                conflicts.push(ModConflict {
                    left_mod_id: left.mod_id.clone(),
                    right_mod_id: Some(right.mod_id.clone()),
                    kind: ConflictKind::MutuallyExclusive,
                    class: ConflictClass::UnsafeRefuse,
                    paths: Vec::new(),
                    detail: "both mods declare the same mutually-exclusive group".into(),
                });
            }
            let overlaps = bounded_overlap(&left.affected_paths, &right.affected_paths);
            if !overlaps.is_empty() {
                let kind = if left.adapter.is_some() && left.adapter == right.adapter {
                    ConflictKind::EmulatorReplacementOverlap
                } else {
                    ConflictKind::PathOverlap
                };
                let class = if explicitly_ordered
                    .get(&left.mod_id)
                    .copied()
                    .unwrap_or(false)
                    && explicitly_ordered
                        .get(&right.mod_id)
                        .copied()
                        .unwrap_or(false)
                {
                    ConflictClass::OrderingRequired
                } else {
                    ConflictClass::NeedsReview
                };
                conflicts.push(ModConflict {
                    left_mod_id: left.mod_id.clone(), right_mod_id: Some(right.mod_id.clone()),
                    kind, class, paths: overlaps,
                    detail: "both enabled mods affect the same destination paths; explicit order is required".into(),
                });
                if class == ConflictClass::NeedsReview {
                    warnings.push(format!(
                        "choose an explicit order for {} and {}",
                        left.mod_id, right.mod_id
                    ));
                }
            }
            if left.derived_output.is_some() && left.derived_output == right.derived_output {
                conflicts.push(ModConflict {
                    left_mod_id: left.mod_id.clone(),
                    right_mod_id: Some(right.mod_id.clone()),
                    kind: ConflictKind::DerivedOutputOverlap,
                    class: ConflictClass::UnsafeRefuse,
                    paths: left.derived_output.clone().into_iter().collect(),
                    detail: "mods produce the same derived output".into(),
                });
            }
            check_patch_chain(left, right, &mut conflicts);
        }
    }
    conflicts.sort_by(|left, right| {
        (
            &left.left_mod_id,
            &left.right_mod_id,
            left.kind,
            &left.paths,
        )
            .cmp(&(
                &right.left_mod_id,
                &right.right_mod_id,
                right.kind,
                &right.paths,
            ))
    });
    conflicts.dedup();
    let refused = conflicts
        .iter()
        .any(|conflict| conflict.class == ConflictClass::UnsafeRefuse)
        || conflicts.iter().any(|conflict| {
            conflict.class == ConflictClass::NeedsReview && operation == ActivationOperation::Enable
        });
    let mut resulting = ModStack {
        game_identity: previous.game_identity.clone(),
        layers,
        generation: previous.generation.saturating_add(1),
    };
    resulting
        .layers
        .sort_by_key(|layer| layer.requested_order.unwrap_or(u32::MAX));
    ActivationPlan {
        operation,
        previous_stack: previous.clone(),
        resulting_stack: resulting,
        conflicts,
        warnings,
        reversible: true,
        refused,
    }
}

fn effective_requested_order(layer: &ModLayer) -> Option<u32> {
    layer.requested_order.or_else(|| {
        layer
            .patch_chain
            .as_ref()
            .and_then(|chain| chain.explicit_order)
    })
}

fn check_patch_chain(left: &ModLayer, right: &ModLayer, conflicts: &mut Vec<ModConflict>) {
    let (Some(a), Some(b)) = (&left.patch_chain, &right.patch_chain) else {
        return;
    };
    if a.chain_id != b.chain_id {
        return;
    }
    if a.explicit_order.is_none() || b.explicit_order.is_none() {
        conflicts.push(ModConflict {
            left_mod_id: left.mod_id.clone(),
            right_mod_id: Some(right.mod_id.clone()),
            kind: ConflictKind::IncompatiblePatchChain,
            class: ConflictClass::UnsafeRefuse,
            paths: Vec::new(),
            detail: "patch-chain stages require explicit order".into(),
        });
        return;
    }
    let (first, second) = if a.explicit_order < b.explicit_order {
        (a, b)
    } else {
        (b, a)
    };
    if let (Some(output), Some(input)) =
        (&first.produced_output_sha256, &second.expected_input_sha256)
        && output != input
    {
        conflicts.push(ModConflict {
            left_mod_id: left.mod_id.clone(),
            right_mod_id: Some(right.mod_id.clone()),
            kind: ConflictKind::IncompatiblePatchChain,
            class: ConflictClass::UnsafeRefuse,
            paths: Vec::new(),
            detail: "patch-chain output does not match the next stage input".into(),
        });
    }
}

fn normalize_path(path: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => parts.push(value.to_string_lossy().replace('\\', "/")),
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) | Component::ParentDir => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/").to_ascii_lowercase())
}

fn bounded_overlap(left: &[PathBuf], right: &[PathBuf]) -> Vec<PathBuf> {
    let left_map = left
        .iter()
        .filter_map(|path| normalize_path(path).map(|key| (key, path)))
        .collect::<BTreeMap<_, _>>();
    let mut result = right
        .iter()
        .filter_map(|path| {
            normalize_path(path).and_then(|key| left_map.get(&key).map(|_| path.clone()))
        })
        .collect::<Vec<_>>();
    result.sort();
    result.dedup();
    result.truncate(MAX_REPORTED_OVERLAPS);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(id: &str, paths: &[&str]) -> ModLayer {
        ModLayer {
            mod_id: id.into(),
            package_id: format!("pkg-{id}"),
            game_identity: "GAME".into(),
            platform: "PSP".into(),
            emulator: Some("PPSSPP".into()),
            mod_type: "texture".into(),
            provenance: ModProvenance {
                source: format!("{id}.zip"),
                content_sha256: Some(id.into()),
                receipt_id: None,
            },
            enabled: true,
            requested_order: None,
            effective_order: None,
            affected_paths: paths.iter().map(PathBuf::from).collect(),
            derived_output: None,
            exclusive_group: None,
            patch_chain: None,
            destination_fingerprint: None,
            current_destination_fingerprint: None,
            transaction_id: None,
            adapter: Some(PreviewAdapter::Ppsspp),
        }
    }

    #[test]
    fn independent_mods_coexist_in_deterministic_order() {
        let mut stack = ModStack::new("GAME");
        let mut b = layer("b", &["b.dds"]);
        b.enabled = false;
        let mut a = layer("a", &["a.dds"]);
        a.enabled = true;
        stack.layers = vec![b, a];
        let plan = stack.preview_enable(stack.layers[0].clone());
        assert!(plan.can_apply());
        assert_eq!(plan.resulting_stack.layers[0].mod_id, "a");
    }

    #[test]
    fn exact_overlap_requires_explicit_order() {
        let stack = ModStack::new("GAME");
        let plan = stack.preview_enable(layer("a", &["ui/hud.dds"]));
        let plan = plan
            .resulting_stack
            .preview_enable(layer("b", &["ui/hud.dds"]));
        assert!(plan.refused);
        assert!(
            plan.conflicts
                .iter()
                .any(|c| c.kind == ConflictKind::EmulatorReplacementOverlap)
        );
    }

    #[test]
    fn explicit_order_allows_proven_overlap() {
        let mut stack = ModStack::new("GAME");
        let mut a = layer("a", &["ui/hud.dds"]);
        a.requested_order = Some(0);
        let mut b = layer("b", &["ui/hud.dds"]);
        b.requested_order = Some(1);
        stack.layers = vec![a];
        let plan = stack.preview_enable(b);
        assert!(plan.can_apply());
        assert_eq!(plan.resulting_stack.layers[0].mod_id, "a");
    }

    #[test]
    fn duplicate_identity_is_refused() {
        let mut stack = ModStack::new("GAME");
        let mut a = layer("same", &["a"]);
        a.enabled = true;
        stack.layers = vec![a.clone()];
        let plan = stack.preview_enable(a);
        assert!(plan.refused);
        assert!(
            plan.conflicts
                .iter()
                .any(|c| c.kind == ConflictKind::DuplicateIdentity)
        );
    }

    #[test]
    fn mutually_exclusive_mods_are_refused() {
        let mut a = layer("a", &["a"]);
        a.exclusive_group = Some("weather".into());
        let mut b = layer("b", &["b"]);
        b.exclusive_group = Some("weather".into());
        let mut stack = ModStack::new("GAME");
        stack.layers = vec![a];
        assert!(stack.preview_enable(b).refused);
    }

    #[test]
    fn compatible_patch_chain_is_explicit_and_accepted() {
        let mut a = layer("a", &["out"]);
        a.patch_chain = Some(PatchChainStage {
            chain_id: "c".into(),
            expected_input_sha256: None,
            produced_output_sha256: Some("b".into()),
            explicit_order: Some(0),
        });
        let mut b = layer("b", &["out"]);
        b.patch_chain = Some(PatchChainStage {
            chain_id: "c".into(),
            expected_input_sha256: Some("b".into()),
            produced_output_sha256: None,
            explicit_order: Some(1),
        });
        let mut stack = ModStack::new("GAME");
        stack.layers = vec![a];
        let plan = stack.preview_enable(b);
        assert!(plan.can_apply());
    }

    #[test]
    fn incompatible_patch_chain_is_refused() {
        let mut a = layer("a", &["out"]);
        a.patch_chain = Some(PatchChainStage {
            chain_id: "c".into(),
            expected_input_sha256: None,
            produced_output_sha256: Some("wrong".into()),
            explicit_order: Some(0),
        });
        let mut b = layer("b", &["out"]);
        b.patch_chain = Some(PatchChainStage {
            chain_id: "c".into(),
            expected_input_sha256: Some("expected".into()),
            produced_output_sha256: None,
            explicit_order: Some(1),
        });
        let mut stack = ModStack::new("GAME");
        stack.layers = vec![a];
        assert!(stack.preview_enable(b).refused);
    }

    #[test]
    fn changed_destination_fails_closed_and_rollback_restores_state() {
        let mut a = layer("a", &["a"]);
        a.destination_fingerprint = Some("old".into());
        a.current_destination_fingerprint = Some("new".into());
        let stack = ModStack::new("GAME");
        let plan = stack.preview_enable(a);
        assert!(plan.refused);
        let receipt = plan.receipt(vec!["tx".into()], 1);
        assert_eq!(receipt.rollback_plan().resulting_stack.layers.len(), 0);
    }

    #[test]
    fn overlap_reporting_is_bounded() {
        let left_paths = (0..(MAX_REPORTED_OVERLAPS + 50))
            .map(|i| PathBuf::from(format!("{i}.bin")))
            .collect::<Vec<_>>();
        let right_paths = left_paths.clone();
        assert_eq!(
            bounded_overlap(&left_paths, &right_paths).len(),
            MAX_REPORTED_OVERLAPS
        );
    }

    #[test]
    fn unsafe_paths_are_refused() {
        let stack = ModStack::new("GAME");
        assert!(stack.preview_enable(layer("a", &["../escape"])).refused);
        assert!(stack.preview_enable(layer("b", &["/absolute"])).refused);
    }
}
